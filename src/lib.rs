#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]

pub mod handlers;

// Serial communication module

use core::ptr::addr_of_mut;
use core::{future::poll_fn, sync::atomic::AtomicBool, task::Poll};

use bbqueue::{self, GrantR};
use critical_section::RestoreState;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::waitqueue::AtomicWaker;
use embedded_io_async::Write;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_hal::Async;
use esp_println::Printer;
use static_cell::{self, make_static};

use esp_hal::macros::ram;

static mut USB_SERIAL_READY: AtomicBool = AtomicBool::new(false);
static mut USB_SERIAL_TX_BUFFER: bbqueue::BBBuffer<2048> = bbqueue::BBBuffer::new();
static mut USB_SERIAL_TX_PRODUCER: Option<bbqueue::Producer<'static, 2048>> = None;
static mut USB_SERIAL_TX_CONSUMER: Option<bbqueue::Consumer<'static, 2048>> = None;

static mut USB_SERIAL_RX_READER: Option<embassy_sync::pipe::Reader<CriticalSectionRawMutex, 256>> =
    None;

static mut WAKER: AtomicWaker = AtomicWaker::new();

static mut ENCODER: defmt::Encoder = defmt::Encoder::new();
static mut CS_RESTORE: RestoreState = RestoreState::invalid();
static mut LOGGER_TAKEN: bool = false;
static mut LOGGER_BUFFER: [u8; 2048] = [0; 2048];
static mut BUFFER_CURSOR: usize = 0;

// defmt::timestamp!("{=u64:us}", {
//     // NOTE(interrupt-safe) single instruction volatile read operation
//     embassy_time::Instant::now().as_micros()
// });

#[defmt::global_logger]
struct GlobalLogger;

unsafe impl defmt::Logger for GlobalLogger {
    #[ram]
    fn acquire() {
        unsafe {
            // SAFETY: Must be paired with corresponding call to release(), see below
            let restore = critical_section::acquire();

            // Compiler fence to prevent reordering of the above critical section with the
            // subsequent access to the `TAKEN` flag.
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

            if LOGGER_TAKEN {
                panic!("defmt logger already taken!");
            }

            // SAFETY: accessing the `static mut` is OK because we have acquired a critical
            // section.
            CS_RESTORE = restore;

            LOGGER_TAKEN = true;
        }

        // SAFETY: this will only be true after the USB serial task has started. Since we are
        // in a critical section, this will be consistent throughout `acquire` and `release`.
        if unsafe { USB_SERIAL_READY.load(core::sync::atomic::Ordering::Relaxed) } {
            // SAFETY: accessing global is safe in a critical section
            unsafe { LOGGER_BUFFER[0..2].copy_from_slice(&[0xFF, 0x00]) };

            let mut size_written = 2;

            // SAFETY: accessing global is safe in a critical section
            unsafe { &mut *addr_of_mut!(ENCODER) }.start_frame(|bytes| {
                if size_written + bytes.len() > unsafe { LOGGER_BUFFER }.len() {
                    // Buffer overflow
                    return;
                }

                // SAFETY: accessing global is safe in a critical section
                unsafe { &mut LOGGER_BUFFER[size_written..size_written + bytes.len()] }
                    .copy_from_slice(bytes);
                size_written += bytes.len();
            });

            // SAFETY: accessing global is safe in a critical section
            unsafe { BUFFER_CURSOR = size_written };
        } else {
            // Early boot, just print to the UART

            // Write a non-UTF8 sequence frame header that is recognized by `espflash`
            do_write(&[0xFF, 0x00]);

            // SAFETY: accessing global is safe in a critical section
            unsafe { ENCODER.start_frame(do_write) }
        }
    }

    #[ram]
    unsafe fn flush() {
        // SAFETY: `WAKER` is guaranteed to be initialized if `USB_SERIAL_READY` is true
        if unsafe { USB_SERIAL_READY.load(core::sync::atomic::Ordering::Relaxed) } {
            // Do nothing
            unsafe {
                WAKER.wake();
            }
        } else {
            // Early boot
            // Printer.flush();
        }
    }

    #[ram]
    unsafe fn release() {
        // SAFETY: branch will only be taken after the USB serial task has started
        if unsafe { USB_SERIAL_READY.load(core::sync::atomic::Ordering::Relaxed) } {
            unsafe { &mut *addr_of_mut!(ENCODER) }.end_frame(|bytes| {
                let cursor = unsafe { BUFFER_CURSOR };

                if cursor + bytes.len() > unsafe { LOGGER_BUFFER }.len() {
                    // Buffer overflow
                    return;
                }

                unsafe { &mut LOGGER_BUFFER[cursor..cursor + bytes.len()] }.copy_from_slice(bytes);
                unsafe { BUFFER_CURSOR += bytes.len() };
            });

            // Write to actual producer
            let cursor = unsafe { BUFFER_CURSOR };
            let bytes = &unsafe { LOGGER_BUFFER }[0..cursor];

            let _ = write_to_usb_serial_buffer(bytes);

            // Wake the serial task
            unsafe { WAKER.wake() };
        } else {
            // safety: accessing the `static mut` is OK because we have acquired a critical
            // section.
            unsafe { &mut *addr_of_mut!(ENCODER) }.end_frame(do_write);

            // Printer.flush();
        }

        unsafe {
            LOGGER_TAKEN = false;

            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

            critical_section::release(CS_RESTORE);
        }
    }

    #[ram]
    unsafe fn write(bytes: &[u8]) {
        if unsafe { USB_SERIAL_READY.load(core::sync::atomic::Ordering::Relaxed) } {
            unsafe { &mut *addr_of_mut!(ENCODER) }.write(bytes, |bytes| {
                let cursor = unsafe { BUFFER_CURSOR };

                if cursor + bytes.len() > unsafe { LOGGER_BUFFER }.len() {
                    // Buffer overflow
                    return;
                }

                unsafe { &mut LOGGER_BUFFER[cursor..cursor + bytes.len()] }.copy_from_slice(bytes);
                unsafe { BUFFER_CURSOR += bytes.len() };
            });
        } else {
            // safety: accessing the `static mut` is OK because we have acquired a critical
            // section.
            unsafe { &mut *addr_of_mut!(ENCODER) }.write(bytes, do_write);
        }
    }
}

#[inline]
fn do_write(bytes: &[u8]) {
    Printer.write_bytes_assume_cs(bytes)
}

/// Write to the USB serial buffer
/// We will also need to acquire the critical section when we write to the USB serial buffer
/// to prevent interleaving with the defmt logger
#[ram]
#[allow(clippy::result_unit_err)]
pub fn write_to_usb_serial_buffer(bytes: &[u8]) -> Result<(), ()> {
    let restore = unsafe { critical_section::acquire() };
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

    let grant = unsafe { USB_SERIAL_TX_PRODUCER.as_mut() }
        .unwrap()
        .grant_exact(bytes.len());

    if let Ok(mut grant) = grant {
        grant.buf().copy_from_slice(bytes);
        grant.commit(bytes.len());

        unsafe { WAKER.wake() };

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        unsafe { critical_section::release(restore) };

        Ok(())
    } else {
        unsafe { WAKER.wake() };

        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        unsafe { critical_section::release(restore) };

        Err(())
    }
}

pub fn reader_take() -> embassy_sync::pipe::Reader<'static, CriticalSectionRawMutex, 256> {
    unsafe { USB_SERIAL_RX_READER.take().unwrap() }
}

/// The serial task
#[embassy_executor::task]
#[ram]
pub async fn serial_comm_task(usb_dev: esp_hal::peripherals::USB_DEVICE) {
    defmt::info!("Serial Task Start!");

    // let peripherals = unsafe { Peripherals::steal() };
    let usb_serial = UsbSerialJtag::<Async>::new_async(usb_dev);

    let serial_rx_queue =
        make_static!(embassy_sync::pipe::Pipe::<CriticalSectionRawMutex, 256>::new());

    // interrupt::enable(Interrupt::USB_DEVICE, interrupt::Priority::Priority1).unwrap();

    let (mut usb_tx, mut usb_rx) = usb_serial.split();

    // Initialize the BBQueue
    let (producer, consumer) = unsafe { USB_SERIAL_TX_BUFFER.try_split().unwrap() };
    unsafe { USB_SERIAL_TX_PRODUCER = Some(producer) };
    unsafe { USB_SERIAL_TX_CONSUMER = Some(consumer) };

    let consumer = unsafe { USB_SERIAL_TX_CONSUMER.as_mut().unwrap() };

    // TX async block
    let tx = async {
        loop {
            let tx_incoming = poll_fn(|cx| -> Poll<Result<GrantR<'_, 2048>, ()>> {
                if let Ok(grant) = consumer.read() {
                    Poll::Ready(Ok(grant))
                } else {
                    unsafe { &*core::ptr::addr_of!(WAKER) }.register(cx.waker());

                    Poll::Pending
                }
            })
            .await;

            if let Ok(grant) = tx_incoming {
                let bytes = grant.buf();

                let len = bytes.len().clamp(0, 64);
                let written = usb_tx.write(&bytes[..len]).await.unwrap();

                grant.release(written);
            }

            embassy_futures::yield_now().await;
        }
    };

    let rx_fut = async {
        let mut buf = [0u8; 64];

        let (recvq_rx, recvq_tx) = serial_rx_queue.split();

        unsafe {
            USB_SERIAL_RX_READER = Some(recvq_rx);
        }

        loop {
            let read_result = embedded_io_async::Read::read(&mut usb_rx, &mut buf).await;

            if read_result.is_err() {
                continue;
            }

            let read_result = read_result.unwrap();

            if read_result == 0 {
                continue;
            }

            let buf = &buf[..read_result];

            defmt::trace!("Received: {:?}", buf);

            // Post the received data to the USB_SERIAL_RX_QUEUE in a write loop
            let mut cursor = 0;

            while cursor < buf.len() {
                match recvq_tx.try_write(&buf[cursor..]) {
                    Ok(written_usize) => {
                        cursor += written_usize;
                    }
                    Err(_) => {
                        // Buffer full, just bail out
                        defmt::error!("USB_SERIAL_RX_QUEUE full!");
                        break;
                    }
                }
            }
        }
    };

    unsafe { USB_SERIAL_READY.store(true, core::sync::atomic::Ordering::Relaxed) };

    // Join the futures
    let tx_rx = embassy_futures::join::join(tx, rx_fut);

    tx_rx.await;
}
