use hyper::{Body, Request, Response, Server, StatusCode};
use hyper::service::{make_service_fn, service_fn};
use std::net::SocketAddr;
use percent_encoding::percent_decode_str;
use image::{DynamicImage, ImageBuffer, Rgba, GenericImageView};
use std::sync::Arc;

struct ImageParams {
    url: String,
    quality: u8,
    grayscale: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));

    let make_svc = make_service_fn(|_conn| async {
        Ok::<_, hyper::Error>(service_fn(handle_request))
    });

    let server = Server::bind(&addr).serve(make_svc);
    println!("Listening on http://{}", addr);
    server.await?;
    Ok(())
}

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
        quality: 80,  // default quality
        grayscale: true,  // default grayscale
    };

    for (key, value) in params {
        match key {
            "url" => image_params.url = percent_decode_str(value).decode_utf8_lossy().to_string(),
            "l" => {
                let parsed_quality = value.parse().unwrap_or(80);
                image_params.quality = parsed_quality.min(100).max(0);
            },
            "bw" => image_params.grayscale = value != "0",
            _ => {}
        }
    }

    image_params
}

fn convert_to_grayscale_optimized(img: &DynamicImage) -> DynamicImage {
    let (width, height) = img.dimensions();
    
    match img {
        DynamicImage::ImageRgba8(rgba_img) => {
            let mut output = ImageBuffer::new(width, height);
            for (x, y, pixel) in rgba_img.enumerate_pixels() {
                let luma = ((pixel[0] as u32 * 299 + pixel[1] as u32 * 587 + pixel[2] as u32 * 114) / 1000) as u8;
                output.put_pixel(x, y, Rgba([luma, luma, luma, pixel[3]]));
            }
            DynamicImage::ImageRgba8(output)
        },
        DynamicImage::ImageRgb8(rgb_img) => {
            let mut output = ImageBuffer::new(width, height);
            for (x, y, pixel) in rgb_img.enumerate_pixels() {
                let luma = ((pixel[0] as u32 * 299 + pixel[1] as u32 * 587 + pixel[2] as u32 * 114) / 1000) as u8;
                output.put_pixel(x, y, Rgba([luma, luma, luma, 255]));
            }
            DynamicImage::ImageRgba8(output)
        },
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

async fn handle_request(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    println!("Received request: {:?}", req.uri());

    if req.uri().path() == "/" && req.uri().query().is_none() {
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("bandwidth-hero-proxy"))
            .unwrap());
    }

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

    println!("Processing image: {} (quality: {}, grayscale: {})", params.url, params.quality, params.grayscale);

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

    let mut img = match image::load_from_memory(&bytes) {
        Ok(img) => img,
        Err(e) => {
            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from(format!("Error processing image: {}", e)))
                .unwrap());
        }
    };

    if params.grayscale {
        img = convert_to_grayscale_optimized(&img);
    }

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
    println!("Successfully processed image");

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "image/webp")
        .body(Body::from(webp_image.to_vec()))
        .unwrap())
}
