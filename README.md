# Rusty Bandwidth

A high-performance image compression proxy server written in Rust. This service can compress and convert images on-the-fly using either WebP or JPEG XL formats, with optional grayscale conversion.

Works perfectly with bandwidth hero browser extension.

## Features

- **Multiple Output Formats**: Supports both WebP and JPEG XL (JXL) encoding
- **Quality Control**: Adjustable compression quality (0-100)
- **Grayscale Conversion**: Optional black and white image conversion
- **Performance Focused**: Written in Rust for optimal speed and memory usage
- **Configurable**: Adjustable port, encoding format, and compression settings

## Installation

## Download it from releases (recommended if you use a linux distro)

https://github.com/furdiburd/rusty-bandwith/releases

### Building from Source

### Prerequisites

- Rust toolchain (1.56 or newer)
- LibWebP development files
- JPEG XL development files

On Ubuntu/Debian:
```bash
sudo apt update
sudo apt install libwebp-dev libjxl-dev
```
### Building the project

1. Clone the repository:
```bash
git clone https://github.com/furdiburd/rusty-bandwith.git
cd rusty-bandwidth
```

2. Build the project:
```bash
cargo build --release
```

The compiled binary will be available at `target/release/rusty-bandwidth`

## Usage

### Starting the Server

Basic usage with default settings (WebP encoding):
```bash
./rusty-bandwidth
```

Using JXL encoding:
```bash
./rusty-bandwidth --jxl
```

### Command Line Options

- `--port <PORT>` or `-p <PORT>`: Set the listening port (default: 8080)
- `--jxl`: Enable JPEG XL encoding instead of WebP
- `--speed <1-8>`: Set JXL encoding speed/effort level (only with --jxl)
  - 1: Fastest encoding, lower quality (Lightning)
  - 8: Slowest encoding, highest quality (Tortoise)
  - Default: 8

### URL Parameters

The proxy accepts the following URL parameters:

- `url`: The URL of the image to process (required)
- `l`: Quality level, 0-100 (default: 80)
- `bw`: Convert to grayscale, 0 or 1 (default: 1)

### Example URLs

1. Basic compression with default settings:
```
http://localhost:8080/?url=https://example.com/image.jpg
```

2. High quality color image:
```
http://localhost:8080/?url=https://example.com/image.jpg&l=90&bw=0
```

3. Low quality grayscale for maximum compression:
```
http://localhost:8080/?url=https://example.com/image.jpg&l=50&bw=1
```

## Format Details

### WebP Mode (Default)

- Supports transparency (alpha channel)
- Most browser support it


### JPEG XL Mode

- Potentially better compression
- Quality settings work inversely (lower numbers = better quality)
- Currently doesn't preserve alpha channel
- Requires browser support for JPEG XL (tested on Firefox nightly, it works)
- Configurable encoding speed for quality/speed tradeoff

## Performance settings

1. **JXL Encoding Speed**:
   - Use lower speed values (1-3) for faster encoding but lower quality
   - Use higher speed values (6-8) for better quality but slower encoding
   - Speed 4-5 provides a balanced trade-off

2. **Quality Settings**:
   - Values 70-80 provide good balance for most images
   - Use 90+ only for images requiring high detail
   - Values below 60 may show visible compression artifacts but size saves are bigger

3. **Memory Usage**:
   - The server processes each image independently
   - Memory usage scales with image dimensions
   - Consider setting up a reverse proxy with rate limiting for production use

## Development

### Project Structure

```
src/
  main.rs          # Main server implementation
Cargo.toml         # Project dependencies and settings
```

### Building for Development

```bash
# Debug build with development features
cargo build

# Run with logging
RUST_LOG=debug cargo run
```

### Running Tests

```bash
cargo test
```

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the GPL-3.0 License - see the LICENSE file for details.
