

use image::{DynamicImage,};
use std::io::Cursor;
use std::io::Read;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use std::convert::Infallible;

#[tokio::main]
async fn main() {
    let addr = ([0, 0, 0, 0], 8080).into();
    
    let make_svc = make_service_fn(|_conn| async {
        Ok::<_, Infallible>(service_fn(handle_request))
    });

    let server = Server::bind(&addr).serve(make_svc);

    println!("Server running at http://localhost:8080"); //i may change the port, it was the first one that got to my mind

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

async fn handle_request(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let url = req.uri().to_string();
    let (image_url, quality, keep_color) = extract_parameters(&url);

    match process_image(&image_url, quality, keep_color).await {
        Ok(processed_image) => {
            Ok(Response::builder()
                .header("Content-Type", "image/avif")
                .body(Body::from(processed_image))
                .unwrap())
        }
        Err(_e) => {
            Ok(Response::builder()
                .body(Body::from("bandwidth-hero-proxy")) //yes, i needed to do this so the extension accept this as the original server
                .unwrap())
        }
    }
}

async fn process_image(url: &str, quality: f32, keep_color: bool) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let url = url.to_string();
    let image_data = tokio::task::spawn_blocking(move || {
        let response = ureq::get(&url).call()?;
        let mut buffer = Vec::new();
        response.into_reader().read_to_end(&mut buffer)?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(buffer)
    }).await??;
    
    let keep_color = keep_color.clone();
    let processed_data = tokio::task::spawn_blocking(move || {
        let img = image::load_from_memory(&image_data)?;
        
        // Convert to grayscale if needed
        let processed_img = if !keep_color {
            DynamicImage::ImageLuma8(img.into_luma8())
        } else {
            img
        };

        let mut output = Vec::new();
        let quality_u8 = quality.clamp(0.0, 100.0) as u8;
        
        let mut cursor = Cursor::new(&mut output);
        let encoder = image::codecs::avif::AvifEncoder::new_with_speed_quality(
            &mut cursor,
            8,  // till i figure out how to make it run with an acceptable speed
            quality_u8
        );
        processed_img.write_with_encoder(encoder)?;

        let color_mode = if keep_color { "color" } else { "b&w" };
        println!("Processing image with quality: {}, color_mode: {}", quality, color_mode);
        println!("Original size: {}, Converted size: {}", image_data.len(), output.len());

        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(output)
    }).await??;

    Ok(processed_data)
}

fn extract_parameters(full_url: &str) -> (String, f32, bool) {
    // Parse the URL and its parameters
    if let Some(query_start) = full_url.find("?url=") {
        let mut quality = 80.0;
        let mut keep_color = false;
        let query_part = &full_url[query_start..];

        // Extract and decode the image URL (everything between ?url= and first & or end)
        let image_url = if let Some(amp_pos) = query_part[5..].find('&') {
            urlencoding::decode(&query_part[5..5+amp_pos])
                .unwrap_or_else(|_| query_part[5..5+amp_pos].to_string().into())
                .into_owned()
        } else {
            urlencoding::decode(&query_part[5..])
                .unwrap_or_else(|_| query_part[5..].to_string().into())
                .into_owned()
        };

        // Check for black and white parameter
        if query_part.contains("&bw=0") {
            keep_color = true;
        }

        // Check for quality parameter
        if let Some(quality_pos) = query_part.find("&l=") {
            let quality_str = &query_part[quality_pos + 3..];
            if let Some(amp_pos) = quality_str.find('&') {
                if let Ok(q) = quality_str[..amp_pos].parse::<f32>() {
                    quality = q;
                }
            } else if let Ok(q) = quality_str.parse::<f32>() {
                quality = q;
            }
        }

        println!("Original query: {}", full_url);
        println!("Image URL: {}", image_url);
        println!("Quality: {}", quality);
        println!("Keep Color: {}", keep_color);

        (image_url, quality, keep_color)
    } else {
        // Fallback for URLs without parameters
        (full_url.to_string(), 80.0, false)
    }
}


