use clap::{Parser, ValueHint};
use hyper::{Body, Request, Response, Server, StatusCode};
use hyper::service::{make_service_fn, service_fn};
use std::net::SocketAddr;
use percent_encoding::percent_decode_str;
use image::{DynamicImage, ImageBuffer, Rgba, GenericImageView};
use std::sync::Arc;
use jpegxl_rs::{encoder_builder, encode::EncoderSpeed, encode::EncoderResult};
use std::path::Path;

// Command line arguments for configuring the server
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Port to listen on
    #[arg(short, long, value_name = "PORT", default_value_t = 8080, value_hint = ValueHint::Other)]
    port: u16,

    /// Enable JXL encoding instead of WebP
    #[arg(long)]
    jxl: bool,

    /// Control JXL encoding speed/effort level
    /// 1 = fastest but lower quality (Lightning)
    /// 8 = slowest but highest quality (Tortoise)
    #[arg(long, value_name = "SPEED", default_value_t = 8)]
    speed: u8,
}

// Parameters extracted from the URL query string
struct ImageParams {
    url: String,
    quality: u8,      // 0-100, where 100 is highest quality
    grayscale: bool,  // Convert to black and white if true
}

// Server configuration that's shared between threads
struct AppConfig {
    use_jxl: bool,
    encoder_speed: EncoderSpeed,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Parse command line arguments
    let args = Args::parse();
    
    // Map the speed argument (1-8) to JXL's encoder speed settings
    // Lower numbers = faster encoding but potentially lower quality
    let speed = match args.speed {
        1 => EncoderSpeed::Lightning,  // Fastest
        2 => EncoderSpeed::Thunder,
        3 => EncoderSpeed::Falcon,
        4 => EncoderSpeed::Cheetah,
        5 => EncoderSpeed::Hare,
        6 => EncoderSpeed::Wombat,
        7 => EncoderSpeed::Squirrel,
        _ => EncoderSpeed::Tortoise,   // Slowest but highest quality
    };

    // Create shared configuration
    let config = Arc::new(AppConfig {
        use_jxl: args.jxl,
        encoder_speed: speed,
    });

    // Set up the server to listen on localhost with the specified port
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));

    println!("Listening on http://{}", addr);
    println!("Image format: {}", if config.use_jxl { "JXL" } else { "WebP" });
    if config.use_jxl {
        println!("JXL encoding speed: {:?}", config.encoder_speed);
    }

    // Create a service that will handle incoming requests
    let config_clone = config.clone();
    let make_svc = make_service_fn(move |_conn| {
        let config = config_clone.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| handle_request(req, config.clone())))
        }
    });

    // Start the server
    let server = Server::bind(&addr).serve(make_svc);
    server.await?;
    Ok(())
}

// Parse query parameters from the URL
// Example URL: /?url=https://example.com/image.jpg&l=80&bw=1
fn parse_query(query: &str) -> ImageParams {
    let params: Vec<(&str, &str)> = query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.split('=');
            match (parts.next(), parts.next()) {
                (Some(key), Some(value)) => Some((key, value)),
                _ => None,
            }
        })
        .collect();

    let mut image_params = ImageParams {
        url: String::new(),
        quality: 80,    // Default to 80% quality
        grayscale: true, // Default to grayscale
    };

    for (key, value) in params {
        match key {
            // The URL of the image to process
            "url" => image_params.url = percent_decode_str(value).decode_utf8_lossy().to_string(),
            // Quality level (l for legacy reasons)
            "l" => {
                let parsed_quality = value.parse().unwrap_or(80);
                image_params.quality = parsed_quality.min(100).max(0);
            },
            // Black and white mode (bw=0 means color, bw=1 means grayscale)
            "bw" => image_params.grayscale = value != "0",
            _ => {}
        }
    }

    image_params
}

// Convert an image to grayscale while preserving alpha channels
fn convert_to_grayscale_optimized(img: &DynamicImage) -> DynamicImage {
    let (width, height) = img.dimensions();
    
    match img {
        // Handle RGBA images (with transparency)
        DynamicImage::ImageRgba8(rgba_img) => {
            let mut output = ImageBuffer::new(width, height);
            for (x, y, pixel) in rgba_img.enumerate_pixels() {
                let luma = ((pixel[0] as u32 * 299 + pixel[1] as u32 * 587 + pixel[2] as u32 * 114) / 1000) as u8;
                output.put_pixel(x, y, Rgba([luma, luma, luma, pixel[3]]));
            }
            DynamicImage::ImageRgba8(output)
        },
        // Handle RGB images (no transparency)
        DynamicImage::ImageRgb8(rgb_img) => {
            let mut output = ImageBuffer::new(width, height);
            for (x, y, pixel) in rgb_img.enumerate_pixels() {
                let luma = ((pixel[0] as u32 * 299 + pixel[1] as u32 * 587 + pixel[2] as u32 * 114) / 1000) as u8;
                output.put_pixel(x, y, Rgba([luma, luma, luma, 255]));
            }
            DynamicImage::ImageRgba8(output)
        },
        // Handle any other image format by converting to RGBA first
        _ => {
            let rgba = img.to_rgba8();
            let mut output = ImageBuffer::new(width, height);
            for (x, y, pixel) in rgba.enumerate_pixels() {
                let luma = ((pixel[0] as u32 * 299 + pixel[1] as u32 * 587 + pixel[2] as u32 * 114) / 1000) as u8;
                output.put_pixel(x, y, Rgba([luma, luma, luma, pixel[3]]));
            }
            DynamicImage::ImageRgba8(output)
        }
    }
}

// Extract filename from URL and change its extension
// Example: "https://example.com/photo.jpg" -> "photo.jxl"
fn get_filename_with_extension(url: &str, new_ext: &str) -> String {
    let path = Path::new(url);
    let stem = path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    format!("{}.{}", stem, new_ext)
}

// Main request handler - processes images based on URL parameters
async fn handle_request(req: Request<Body>, config: Arc<AppConfig>) -> Result<Response<Body>, hyper::Error> {
    println!("Received request: {:?}", req.uri());

    // Handle root path - show "bandwidth-hero-proxy" to make it work with the extension
    if req.uri().path() == "/" && req.uri().query().is_none() {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("bandwidth-hero-proxy"))
            .unwrap());
    }

    // Make sure we have query parameters
    let query = match req.uri().query() {
        Some(q) => q,
        _none => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("Missing query parameters. Use /?url=<image_url>&bw=<0|1>&l=<0-100>"))
                .unwrap());
        }
    };

    let params = parse_query(query);
    if params.url.is_empty() {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("Missing image URL"))
            .unwrap());
    }

    println!("Processing image: {} (quality: {}, grayscale: {}, format: {})", 
        params.url, params.quality, params.grayscale, if config.use_jxl { "JXL" } else { "WebP" });

    // Download the image
    let response = match reqwest::get(&params.url).await {
        Ok(response) => response,
        Err(e) => {
            println!("Error fetching image: {}", e);
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from(format!("Error fetching image: {}", e)))
                .unwrap());
        }
    };

    let status = response.status();
    if !status.is_success() {
        return Ok(Response::builder()
            .status(status)
            .body(Body::from(format!("Error fetching image: {}", status)))
            .unwrap());
    }

    // Get the image data
    let bytes = Arc::new(match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            println!("Error reading image data: {}", e);
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Error reading image: {}", e)))
                .unwrap());
        }
    });

    // Load and decode the image
    let mut img = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        Err(e) => {
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Error processing image: {}", e)))
                .unwrap());
        }
    };

    // Convert to grayscale if requested
    if params.grayscale {
        img = convert_to_grayscale_optimized(&img);
    }

    if config.use_jxl {
        // JXL quality is inverse of standard quality:
        // - Lower numbers mean better quality (0 is lossless)
        // - Higher numbers mean more compression
        let jxl_quality = if params.quality >= 95 {
            0.0  // Use lossless mode for very high quality requests
        } else {
            let normalized = params.quality as f32 / 100.0;
            // Use exponential curve to make quality changes more gradual
            // This gives better quality preservation at lower input values
            8.0 * (1.0 - normalized.powf(0.7))
        };
        
        // Create JXL encoder with the configured speed
        let mut encoder = match encoder_builder()
            .speed(config.encoder_speed)
            .build() {
                Ok(encoder) => encoder,
                Err(e) => {
                    println!("JXL encoder creation error: {}", e);
                    return Ok(Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from(format!("JXL encoder creation error: {}", e)))
                        .unwrap());
                }
            };

        encoder.quality = jxl_quality;
        encoder.lossless = params.quality >= 95;
        
        // Convert to RGB for JXL encoding
        // Note: This drops alpha channel support for now
        let rgb = img.to_rgb8();
        let raw_pixels: Vec<u8> = rgb.into_raw();
        
        let encoded: EncoderResult<u8> = match encoder.encode(
            &raw_pixels,
            img.width(),
            img.height()
        ) {
            Ok(encoded) => encoded,
            Err(e) => {
                println!("JXL encoding error: {}", e);
                return Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from(format!("JXL encoding error: {}", e)))
                    .unwrap());
            }
        };

        println!("Successfully processed image as JXL");

        // Return the JXL image
        let filename = get_filename_with_extension(&params.url, "jxl");
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "image/jxl")
            .header("Content-Disposition", format!("inline; filename=\"{}\"", filename))
            .body(Body::from(encoded.data))
            .unwrap())
    } else {
        // WebP encoding - quality is straightforward 0-100
        let quality_float = params.quality as f32;
        let webp_encoder = match webp::Encoder::from_image(&img) {
            Ok(encoder) => encoder,
            Err(e) => {
                println!("WebP encoding error: {}", e);
                return Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Body::from(format!("WebP encoding error: {}", e)))
                    .unwrap());
            }
        };

        let webp_image = webp_encoder.encode(quality_float);
        println!("Successfully processed image as WebP");

        // Return the WebP image
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "image/webp")
            .body(Body::from(webp_image.to_vec()))
            .unwrap())
    }
}
