use image::DynamicImage;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use std::convert::Infallible;
use std::sync::Arc;
use moka::future::Cache;
use lazy_static::lazy_static;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT, ACCEPT, ACCEPT_LANGUAGE};
use std::io::Cursor;
use clap::Parser;
use std::sync::atomic::{AtomicUsize, Ordering};
use threadpool::ThreadPool;
use crossbeam_channel::{bounded, Sender};
use std::thread;
use std::time::Duration;
use byte_unit::Byte;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Port number to run the server on
    #[arg(short, long, default_value_t = 8080)]
    port: u16,

    /// Use WebP instead of AVIF
    #[arg(long)]
    webp: bool,

    /// Number of processing cores to use
    #[arg(short, long, default_value_t = num_cpus::get())]
    cores: usize,

    /// Maximum cache size (e.g., "512MB", "1GB", "2.5GB")
    #[arg(long, default_value = "512MB")]
    cache_size: String,
}

lazy_static! {
    static ref CLIENT: reqwest::Client = {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36"));
        headers.insert(ACCEPT, HeaderValue::from_static("image/*"));
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));

        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(5)
            .default_headers(headers)
            .build()
            .unwrap()
    };
}

struct Task {
    url: String,
    quality: f32,
    keep_color: bool,
    use_webp: bool,
    response_sender: Sender<Result<Vec<u8>, String>>,
}

struct AppState {
    cache: Cache<String, Vec<u8>>,
    use_webp: bool,
    task_sender: Sender<Task>,
    queued_tasks: Arc<AtomicUsize>,
    cache_hits: Arc<AtomicUsize>,
    cache_misses: Arc<AtomicUsize>,
    cache_memory_used: Arc<AtomicUsize>,
    max_cache_size: usize,
}
impl AppState {
    fn new(use_webp: bool, core_count: usize, cache_size: &str) -> Result<Self, Box<dyn std::error::Error>> {
        // Parse cache size string (e.g., "512MB", "1GB")
        let cache_bytes = Byte::from_str(cache_size)
            .map_err(|_| "Invalid cache size format")?
            .get_bytes() as usize;

        let cache = Cache::builder()
            .max_capacity(10000)  // Set a high number of items, we'll control by memory instead
            .time_to_live(Duration::from_secs(3600))
            .time_to_idle(Duration::from_secs(1800))
            .build();

        let (task_sender, task_receiver) = bounded::<Task>(1000);
        let queued_tasks = Arc::new(AtomicUsize::new(0));
        let qt_clone = queued_tasks.clone();
        
        let cache_hits = Arc::new(AtomicUsize::new(0));
        let cache_misses = Arc::new(AtomicUsize::new(0));
        let cache_memory_used = Arc::new(AtomicUsize::new(0));
        let max_cache_size = cache_bytes;

        let pool = ThreadPool::new(core_count);
        println!("Started processing pool with {} cores", core_count);

        thread::spawn(move || {
            while let Ok(task) = task_receiver.recv() {
                let qt = qt_clone.clone();
                
                pool.execute(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();

                    let result = rt.block_on(async {
                        process_image(
                            &task.url,
                            task.quality as u8,
                            task.keep_color,
                            task.use_webp,
                        ).await
                    });

                    let _ = task.response_sender.send(result.map_err(|e| e.to_string()));
                    let current = qt.fetch_sub(1, Ordering::SeqCst);
                    println!("Task completed. Remaining tasks in queue: {}", current - 1);
                });
            }
        });

        Ok(Self {
            cache,
            use_webp,
            task_sender,
            queued_tasks,
            cache_hits,
            cache_misses,
            cache_memory_used,
            max_cache_size,
        })
    }

    fn print_cache_stats(&self) {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            (hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        
        let memory_used = self.cache_memory_used.load(Ordering::Relaxed);
        let memory_used_mb = memory_used as f64 / 1024.0 / 1024.0;
        let max_cache_mb = self.max_cache_size as f64 / 1024.0 / 1024.0;
        
        println!("Cache statistics:");
        println!("  Items in cache: {}", self.cache.entry_count());
        println!("  Memory used: {:.1} MB / {:.1} MB ({:.1}%)", 
            memory_used_mb,
            max_cache_mb,
            (memory_used_mb / max_cache_mb) * 100.0);
        println!("  Hits: {}", hits);
        println!("  Misses: {}", misses);
        println!("  Hit rate: {:.1}%", hit_rate);
    }

    async fn try_add_to_cache(&self, key: String, data: Vec<u8>) -> bool {
        let data_size = data.len();
        
        // Check if adding this item would exceed the cache size limit
        let current_size = self.cache_memory_used.load(Ordering::Relaxed);
        if current_size + data_size > self.max_cache_size {
            // Try to make room by removing old items
            let entries: Vec<_> = self.cache.iter()
                .take(5)
                .collect();
    
            for (k, v) in entries {
                self.cache_memory_used.fetch_sub(v.len(), Ordering::Relaxed);
                // Convert Arc<String> to &str for invalidation
                self.cache.invalidate(k.as_str()).await;
            }
            
            // Check again if we have room now
            let new_size = self.cache_memory_used.load(Ordering::Relaxed);
            if new_size + data_size > self.max_cache_size {
                return false; // Still no room
            }
        }
    
        // Add to cache and update memory usage
        self.cache_memory_used.fetch_add(data_size, Ordering::Relaxed);
        self.cache.insert(key, data).await;
        true
    }
    
}

async fn handle_request(req: Request<Body>, state: Arc<AppState>) -> Result<Response<Body>, Infallible> {
    let url = req.uri().to_string();
    let (image_url, quality, keep_color) = extract_parameters(&url);

    let content_type = if state.use_webp { "image/webp" } else { "image/avif" };

    let response_builder = Response::builder()
        .header("Access-Control-Allow-Origin", "*")
        .header("Access-Control-Allow-Methods", "GET, OPTIONS")
        .header("Access-Control-Allow-Headers", "Content-Type")
        .header("Content-Type", content_type);

    if req.method() == hyper::Method::OPTIONS {
        return Ok(response_builder
            .body(Body::empty())
            .unwrap());
    }

    let cache_key = format!("{}:{}:{}:{}", image_url, quality, keep_color, state.use_webp);

    // Try to get from cache
    if let Some(cached_image) = state.cache.get(&cache_key) {
        state.cache_hits.fetch_add(1, Ordering::Relaxed);
        state.print_cache_stats();
        return Ok(response_builder
            .header("X-Cache", "HIT")
            .header("Content-Length", cached_image.len().to_string())
            .body(Body::from(cached_image))
            .unwrap());
    }

    // Create response channel
    let (response_sender, response_receiver) = bounded(1);

    // Create task
    let task = Task {
        url: image_url.clone(),
        quality,
        keep_color,
        use_webp: state.use_webp,
        response_sender,
    };

    // Update and print queue count
    let current_queue = state.queued_tasks.fetch_add(1, Ordering::SeqCst);
    println!("New task queued. Current queue size: {}", current_queue + 1);

    // Send task to processing queue
    if let Err(e) = state.task_sender.send(task) {
        return Ok(Response::builder()
            .status(500)
            .body(Body::from(format!("Failed to queue task: {}", e)))
            .unwrap());
    }

    // Wait for the result
    match response_receiver.recv() {
        Ok(Ok(processed_image)) => {
            // Try to store in cache
            if state.try_add_to_cache(cache_key, processed_image.clone()).await {
                println!("Image added to cache");
            } else {
                println!("Image too large for cache");
            }
            
            state.print_cache_stats();
            
            Ok(response_builder
                .header("X-Cache", "MISS")
                .header("Content-Length", processed_image.len().to_string())
                .body(Body::from(processed_image))
                .unwrap())
        }
        Ok(Err(e)) => {
            if e.contains("relative URL without a base") {
                // Return bandwidth-hero-proxy server response without incrementing cache miss
                Ok(Response::builder()
                    .status(200)
                    .header("Content-Type", "text/plain")
                    .body(Body::from("bandwidth-hero-proxy"))
                    .unwrap())
            } else {
                // For other errors, increment cache miss and return error response
                state.cache_misses.fetch_add(1, Ordering::Relaxed);
                Ok(Response::builder()
                    .status(500)
                    .body(Body::from(format!("Error processing image: {}", e)))
                    .unwrap())
            }
        }
        Err(e) => {
            println!("Error receiving processed image: {}", e);
            state.cache_misses.fetch_add(1, Ordering::Relaxed);
            Ok(Response::builder()
                .status(500)
                .body(Body::from("Internal processing error"))
                .unwrap())
        }
    }
}


fn extract_parameters(full_url: &str) -> (String, f32, bool) {
    if let Some(query_start) = full_url.find("?url=") {
        let mut quality = 80.0;
        let mut keep_color = false;
        let query_part = &full_url[query_start..];

        let image_url = if let Some(amp_pos) = query_part[5..].find('&') {
            urlencoding::decode(&query_part[5..5+amp_pos])
                .unwrap_or_else(|_| query_part[5..5+amp_pos].to_string().into())
                .into_owned()
        } else {
            urlencoding::decode(&query_part[5..])
                .unwrap_or_else(|_| query_part[5..].to_string().into())
                .into_owned()
        };

        if query_part.contains("&bw=0") {
            keep_color = true;
        }

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
        (full_url.to_string(), 80.0, false)
    }
}

async fn process_image(
    url: &str,
    quality: u8,
    keep_color: bool,
    use_webp: bool,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    println!("Processing image on thread {:?}", std::thread::current().id());

    let parsed_url = url::Url::parse(url)?;
    let host = parsed_url.host_str().unwrap_or("");

    let response = CLIENT.get(url)
        .header("Referer", format!("https://{}/", host))
        .header("Origin", format!("https://{}", host))
        .header("Sec-Fetch-Dest", "image")
        .header("Sec-Fetch-Mode", "no-cors")
        .header("Sec-Fetch-Site", "cross-site")
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(format!("Failed to fetch image: HTTP {}", response.status()).into());
    }
    let img_bytes = response.bytes().await?;
    let mut img = image::load_from_memory(&img_bytes)?;

    // Convert to grayscale if not keeping color
    if !keep_color {
        img = DynamicImage::ImageLuma8(img.to_luma8());
    }

    let mut buffer = Vec::new();
    if use_webp {
        // Create WebP encoder with transparency support
        let encoder = webp::Encoder::from_image(&img).map_err(|e| e.to_string())?;
        let encoded = encoder
            .encode(quality as f32); // quality from 0-100
            //.map_err(|e| e.to_string())?;
        
        buffer.extend_from_slice(&encoded);
    } else {
        // For non-WebP format (e.g., JPEG), use standard encoding
        let encoder = webp::Encoder::from_image(&img).map_err(|e| e.to_string())?;
        let encoded = encoder
            .encode(quality as f32); // quality from 0-100
        buffer.extend_from_slice(&encoded);
    }

    Ok(buffer)
}


#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    
    let addr = ([0, 0, 0, 0], args.port).into();
    let state = Arc::new(AppState::new(args.webp, args.cores, &args.cache_size)?);
    
    println!("Server running at http://localhost:{}", args.port);
    println!("Using {} format with {} cores", 
        if args.webp { "WebP" } else { "AVIF" },
        args.cores);
    println!("Cache size: {}", args.cache_size);

    let make_svc = make_service_fn(|_conn| {
        let state = Arc::clone(&state);
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                let state = Arc::clone(&state);
                handle_request(req, state)
            }))
        }
    });

    let server = Server::bind(&addr).serve(make_svc);

    println!("Server is ready to accept connections");
    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }

    Ok(())
}
