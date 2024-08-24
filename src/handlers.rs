use esp_hal;

#[cfg(any(feature = "esp32s3"))]
fn halt_core(core: esp_hal::Cpu) {
    #[cfg(feature = "esp32p4")]
    mod registers {
        pub(crate) const SW_CPU_STALL: u32 = 0x50115200;
    }

    #[cfg(feature = "esp32s3")]
    mod registers {
        pub(crate) const OPTIONS0: u32 = 0x60008000;
        pub(crate) const SW_CPU_STALL: u32 = 0x600080bc;
    }

    let sw_cpu_stall = registers::SW_CPU_STALL as *mut u32;

    unsafe {
        if core == esp_hal::Cpu::AppCpu {
            // CPU0
            sw_cpu_stall
                .write_volatile(sw_cpu_stall.read_volatile() & !(0b111111 << 20) | (0x21 << 20));

            let options0 = registers::OPTIONS0 as *mut u32;

            options0.write_volatile(options0.read_volatile() & !(0b0011) | 0b0010);
        } else {
            sw_cpu_stall
                .write_volatile(sw_cpu_stall.read_volatile() & !(0b111111 << 26) | (0x21 << 26));

            let options0 = registers::OPTIONS0 as *mut u32;

            options0.write_volatile(options0.read_volatile() & !(0b1100) | 0b1000);
        }
    }
}

/// Custom halt function
///
/// This function is called when the program panics. We use this instead of the default panic handler
/// to allow us to flush the log buffer to the USB serial port before halting the CPU.
#[export_name = "custom_halt"]
pub fn custom_halt() -> ! {
    #[cfg(feature = "esp32p4")]
    unsafe {}

    #[cfg(any(feature = "esp32s3"))]
    {
        // We need to write the value "0x86" to stall a particular core. The write
        // location is split into two separate bit fields named "c0" and "c1", and the
        // two fields are located in different registers. Each core has its own pair of
        // "c0" and "c1" bit fields.

        // First get which core we are running on
        let current_core = esp_hal::get_core();

        if current_core == esp_hal::Cpu::ProCpu {
            halt_core(esp_hal::Cpu::AppCpu);
        } else {
            halt_core(esp_hal::Cpu::ProCpu);
        }
    }

    // Enter a critical section to prevent interrupts from firing
    // We will never exit this critical section
    let _ = unsafe { critical_section::acquire() };

    // Try reading from the USB serial buffer
    if let Some(consumer) = unsafe { super::USB_SERIAL_TX_CONSUMER.as_mut() } {
        if let Ok(grant) = consumer.read() {
            super::do_write(grant.buf());
        }
    }

    // TODO: enable this after `core::arch::asm` is stabilized
    // wait interrupt forever instead of halting the CPU
    //
    // NOTE: if the core is halted the USB serial flasher will not work
    // unsafe { core::arch::asm!("waiti 0") };

    // Infinite loop but don't stall the CPU
    #[allow(clippy::empty_loop)]
    loop {}
}
