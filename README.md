# `esp-fast-serial`

Fast serial communication for ESP32-S3 and similar microcontrollers that has a built-in USB Serial JTAG interface.

## Features

- Much faster than the default `esp-println` implementation.
- Uses the built-in USB Serial JTAG interface.
- No need for external hardware.
- Provides a fast concurrent `defmt` printer.
- Provides a direct write function for ASCII/raw data alongside `defmt` logging.

## Usage

Add the following to your `Cargo.toml`:

```toml
[dependencies]
esp-fast-serial = { version = "0.1.0", features = ["esp32s3"] }
```

Then, in your `main.rs`:

```rust
spawner.spawn(esp_fast_serial::serial_comm_task(peripherals.USB_DEVICE));
```

## License

MIT OR Apache-2.0
