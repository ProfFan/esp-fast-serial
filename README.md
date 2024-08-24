# `esp-fast-serial`

Fast serial communication for ESP32-S3 and similar microcontrollers that has a built-in USB Serial JTAG interface.

## Features

- Much faster than the default `esp-println` implementation.
- Uses the built-in USB Serial JTAG interface.
- No need for external hardware.
- Provides a fast concurrent `defmt` printer.
- Provides a direct write function for ASCII/raw data alongside `defmt` logging.
- Provides a custom halt function that allows reflashing without disconnecting the USB cable.

## Limitations

- Only supports `defmt` messages that are less than 2048 bytes.
  - This is due to the current implementation of the "global" logger.
  - As `defmt` messages cannot be interleaved, we have to make a global buffer to store the full message.
  - It is possible to create your own local logger that can handle larger messages.
- Currently only supports the S3 and the C6

## Usage

Add the following to your `Cargo.toml`:

```toml
[dependencies]
esp-fast-serial = { version = "0.2.2", features = ["esp32s3"] }
```

Then, in your `main.rs`:

```rust
spawner.spawn(esp_fast_serial::serial_comm_task(peripherals.USB_DEVICE));
```

## License

MIT OR Apache-2.0
