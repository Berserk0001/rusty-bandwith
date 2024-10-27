use image::{DynamicImage, ImageFormat};
use std::io::Cursor;
use tiny_http::{Response, Server, Header};
use std::io::Read;

fn main() {
    let server = Server::http("0.0.0.0:8080").unwrap();
    println!("Server running at http://localhost:8080");

    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let (image_url, quality, keep_color) = extract_parameters(&url);

        match process_image(&image_url, quality, keep_color) {
            Ok(processed_image) => {
                let header = Header::from_bytes("Content-Type", "image/avif")
                    .unwrap();
                let response = Response::from_data(processed_image)
                    .with_header(header);
                let _ = request.respond(response);
            }
            Err(e) => {
                println!("Error processing image: {}", e);
                let response = Response::from_string(format!("Error: {}", e))
                    .with_status_code(500);
                let _ = request.respond(response);
            }
        }
    }
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

        (image_url, quality.clamp(1.0, 100.0), keep_color)
    } else {
        // Fallback for URLs without parameters
        (full_url.to_string(), 80.0, false)
    }
}

fn process_image(url: &str, quality: f32, keep_color: bool) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    // Fetch the image
    let response = ureq::get(url).call()?;
    let mut buffer = Vec::new();
    response.into_reader().read_to_end(&mut buffer)?;
    
    // Load the image
    let img = image::load_from_memory(&buffer)?;
    
    // Convert to grayscale if needed
    let processed_img = if !keep_color {
        DynamicImage::ImageLuma8(img.into_luma8())
    } else {
        img
    };

    // Convert to AVIF
    let mut output = Vec::new();
    processed_img.write_to(&mut Cursor::new(&mut output), ImageFormat::Avif)?;

    let color_mode = if keep_color { "color" } else { "b&w" };
    println!("Processing image with quality: {}, color_mode: {}", quality, color_mode);
    println!("Original size: {}, Converted size: {}", buffer.len(), output.len());

    Ok(output)
}
