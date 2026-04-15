use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{bounded, unbounded};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_store::StoreExt;
use tonic::{
    transport::{Certificate, Channel, ClientTlsConfig},
    Request,
};
use url::Url;
use uuid::Uuid;

use md5rs::md5rs_client::Md5rsClient;
use md5rs::{AuthRequest, AuthResponse, DetectRequest, HealthRequest};

pub mod md5rs {
    tonic::include_proto!("md5rs");
}

pub mod export;
pub mod io;
pub mod media;
pub mod utils;

pub use export::{export_worker, parse_export_csv, Bbox, ExportFrame};
pub use media::{media_worker, WebpItem};
pub use utils::FileItem;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectOptions {
    pub selected_folder: String,
    pub grpc_url: String,
    pub access_token: String,
    pub resume_path: Option<String>,
    pub guess: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOptions {
    pub confidence_threshold: f32,
    pub iou_threshold: f32,
    pub quality: f32,
    pub export_format: ExportFormat,
    pub max_frames: Option<usize>,
    pub iframe_only: bool,
    pub check_point: usize,
    pub buffer_path: Option<String>,
    pub buffer_size: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub detect_options: DetectOptions,
    pub config_options: ConfigOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum ExportFormat {
    Json,
    Csv,
}

async fn create_grpc_client(grpc_url: &str) -> Result<Channel> {
    let url = Url::parse(grpc_url)?;

    // 创建 channel builder
    let mut channel_builder = Channel::from_shared(url.to_string()).context("Invalid URL")?;

    // 仅在 HTTPS 时应用 TLS 配置
    if url.scheme() == "https" {
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("Missing host in URL"))?;

        // 检查主机是否为 IP 地址
        let is_ip_addr = host.parse::<std::net::IpAddr>().is_ok();

        // 获取 TLS 证书并配置 TLS
        let pem = utils::get_tls_certificate(grpc_url)?;
        let ca = Certificate::from_pem(pem);

        // 对 IP 地址可能需要特殊处理域名验证
        let tls = if is_ip_addr {
            ClientTlsConfig::new().ca_certificate(ca).domain_name(host) // 仍然需要 SNI
        } else {
            ClientTlsConfig::new().ca_certificate(ca).domain_name(host)
        };

        channel_builder = channel_builder
            .tls_config(tls)
            .context("Failed to configure TLS")?;
    }

    // 连接到服务器
    channel_builder
        .connect()
        .await
        .context("Failed to connect to server")
}

async fn process(config: Config, progress_sender: crossbeam_channel::Sender<usize>) -> Result<()> {
    let channel = create_grpc_client(&config.detect_options.grpc_url).await?;

    let mut client = Md5rsClient::new(channel);
    let auth_response = auth(&mut client, &config.detect_options.access_token).await?;

    let session_token = auth_response.token;

    cleanup_buffer(&config.config_options.buffer_path)?;

    if config.config_options.check_point == 0 {
        log::error!("Checkpoint should be greater than 0");
        return Ok(());
    }

    let folder_path = std::path::PathBuf::from(&config.detect_options.selected_folder);
    let folder_path = std::fs::canonicalize(folder_path)?;

    let imgsz = 1280;
    let start = Instant::now();

    let mut file_paths = utils::index_files_and_folders(&folder_path)?;

    let export_data = Arc::new(Mutex::new(Vec::new()));
    let frames = Arc::new(Mutex::new(HashMap::<String, ExportFrame>::new()));

    let file_paths = match config.detect_options.resume_path {
        Some(checkpoint_path) => {
            let resume_path = &checkpoint_path.trim().to_string();
            if resume_path != "" {
                let all_files =
                    resume_from_checkpoint(&resume_path, &mut file_paths, &export_data)?;
                all_files.to_owned()
            } else {
                file_paths
            }
        }
        None => file_paths,
    };

    let (media_q_s, media_q_r) = bounded(8);
    let (io_q_s, io_q_r) = bounded(config.config_options.buffer_size);
    let (export_q_s, export_q_r) = unbounded();
    let checkpoint_counter = Arc::new(Mutex::new(0 as usize));
    let progress_sender_clone = progress_sender.clone();
    let cancel_flag = Arc::new(AtomicBool::new(false));

    let buffer_path = config.config_options.buffer_path.clone();
    let folder_path_clone = folder_path.clone();
    let export_data_clone = Arc::clone(&export_data);
    let finish = Arc::new(Mutex::new(false));
    let finish_clone = Arc::clone(&finish);

    thread::spawn(move || {
        let export_data = Arc::clone(&export_data);
        let folder_path = folder_path.clone();
        let checkpoint_counter = Arc::clone(&checkpoint_counter);
        export_worker(
            config.config_options.check_point,
            &checkpoint_counter,
            &config.config_options.export_format,
            &folder_path,
            export_q_r,
            &export_data,
        );
        let mut finish_lock = finish.lock().unwrap();
        *finish_lock = true;
    });

    if let Some(buffer_path) = buffer_path {
        let cancel_flag_clone = Arc::clone(&cancel_flag);
        let export_q_s_for_media = export_q_s.clone();
        rayon::spawn(move || {
            std::fs::create_dir_all(&buffer_path).unwrap();
            let buffer_path = std::fs::canonicalize(buffer_path).unwrap();

            let cancel_for_io = Arc::clone(&cancel_flag_clone);
            let io_handle = thread::spawn(move || {
                for file in file_paths.iter() {
                    if cancel_for_io.load(Ordering::Relaxed) {
                        break;
                    }
                    io::io_worker(&buffer_path, file, io_q_s.clone()).unwrap();
                }
                drop(io_q_s);
            });

            io_q_r.iter().par_bridge().for_each(|file| {
                if cancel_flag_clone.load(Ordering::Relaxed) {
                    return;
                }
                media_worker(
                    file,
                    imgsz,
                    config.config_options.quality,
                    config.config_options.iframe_only,
                    config.config_options.max_frames,
                    media_q_s.clone(),
                    export_q_s_for_media.clone(),
                    progress_sender_clone.clone(),
                    Arc::clone(&cancel_flag_clone),
                );
            });
            io_handle.join().unwrap();
        });
    } else {
        let cancel_flag_clone = Arc::clone(&cancel_flag);
        let export_q_s_for_media = export_q_s.clone();
        rayon::spawn(move || {
            file_paths.par_iter().for_each(|file| {
                if cancel_flag_clone.load(Ordering::Relaxed) {
                    return;
                }
                media_worker(
                    file.clone(),
                    imgsz,
                    config.config_options.quality,
                    config.config_options.iframe_only,
                    config.config_options.max_frames,
                    media_q_s.clone(),
                    export_q_s_for_media.clone(),
                    progress_sender_clone.clone(),
                    Arc::clone(&cancel_flag_clone),
                );
            });
            drop(media_q_s);
        });
    }

    let frames_clone = Arc::clone(&frames);
    let export_q_s_clone = export_q_s.clone();
    let outbound = async_stream::stream! {
        while let Ok(item) = media_q_r.recv() {
            match item {
                WebpItem::Frame(frame) => {
                    let uuid = Uuid::new_v4().to_string();
                    let export_frame = ExportFrame {
                        file: frame.file.clone(),
                        frame_index: frame.frame_index,
                        shoot_time: frame.shoot_time.map(|t| t.to_string()),
                        total_frames: frame.total_frames,
                        iframe: frame.iframe,
                        bboxes: None,
                        label: None,
                        error: None,
                    };
                    frames_clone.lock().unwrap().insert(uuid.clone(), export_frame);
                    yield DetectRequest { uuid, image: frame.webp, width: frame.width as i32, height: frame.height as i32, iou: config.config_options.iou_threshold, score: config.config_options.confidence_threshold, iframe:frame.iframe };
                }
                WebpItem::ErrFile(file) => {
                    export_q_s_clone.send(ExportFrame {
                        file: file.file.clone(),
                        frame_index: 0,
                        shoot_time: None,
                        total_frames: 0,
                        iframe: false,
                        bboxes: None,
                        label: None,
                        error: Some(file.error.to_string()),
                    }).unwrap();
                }
            }
        }
    };

    let mut request = Request::new(outbound);
    request
        .metadata_mut()
        .insert("authorization", session_token.parse().unwrap());

    let response = client.detect(request).await;
    let mut inbound = match response {
        Ok(response) => response.into_inner(),
        Err(status) => {
            log::error!("{}", status.message());
            cleanup_buffer(&config.config_options.buffer_path)?;
            return Ok(());
        }
    };

    loop {
        match inbound.message().await {
            Ok(Some(response)) => {
                let uuid = response.uuid.clone();
                let mut frames = frames.lock().unwrap();
                if let Some(mut frame) = frames.remove(&uuid) {
                    frame.bboxes = Some(
                        response
                            .bboxs
                            .into_iter()
                            .map(|bbox| Bbox {
                                x1: bbox.x1,
                                y1: bbox.y1,
                                x2: bbox.x2,
                                y2: bbox.y2,
                                class: bbox.class as usize,
                                score: bbox.score,
                            })
                            .collect(),
                    );
                    frame.label = Some(response.label);
                    export_q_s.send(frame).unwrap();
                }
            }
            Ok(None) => {
                drop(export_q_s);
                while !*finish_clone.lock().unwrap() {
                    thread::sleep(Duration::from_millis(100));
                }
                export::export(
                    &folder_path_clone,
                    export_data_clone,
                    &config.config_options.export_format,
                )?;
                cleanup_buffer(&config.config_options.buffer_path)?;
                break;
            }
            Err(e) => {
                log::error!("Error receiving detection: {}", e);
                cancel_flag.store(true, Ordering::Relaxed);
                drop(export_q_s);
                while !*finish_clone.lock().unwrap() {
                    thread::sleep(Duration::from_millis(100));
                }
                export::export(
                    &folder_path_clone,
                    export_data_clone,
                    &config.config_options.export_format,
                )?;
                cleanup_buffer(&config.config_options.buffer_path)?;
                break;
            }
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(anyhow::anyhow!(
            "Processing stopped because media channel was disconnected"
        ));
    }

    log::info!("Elapsed time: {:?}", start.elapsed());
    Ok(())
}

async fn auth(client: &mut Md5rsClient<Channel>, token: &str) -> Result<AuthResponse> {
    let response = client
        .auth(Request::new(AuthRequest {
            token: token.to_string(),
        }))
        .await?;
    let auth_response = response.into_inner();
    if auth_response.success {
        Ok(auth_response)
    } else {
        Err(anyhow::anyhow!("Auth failed"))
    }
}

async fn get_auth(grpc_url: String, token: String) -> Result<i32> {
    let channel = create_grpc_client(&grpc_url).await?;
    let mut client = Md5rsClient::new(channel);

    match auth(&mut client, &token).await {
        Ok(response) => Ok(response.quota),
        Err(_) => Err(anyhow::anyhow!("Auth failed")),
    }
}

async fn health(client: &mut Md5rsClient<Channel>) -> Result<()> {
    let response = client.health(Request::new(HealthRequest {})).await?;
    let health_response = response.into_inner();
    if health_response.status {
        Ok(())
    } else {
        log::error!("Health check failed");
        Err(anyhow::anyhow!("Check failed"))
    }
}

async fn get_health(grpc_url: String) -> Result<bool> {
    let channel = create_grpc_client(&grpc_url).await?;
    let mut client = Md5rsClient::new(channel);

    match health(&mut client).await {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn cleanup_buffer(buffer_path: &Option<String>) -> Result<()> {
    if let Some(path) = buffer_path {
        let path = std::path::PathBuf::from(path);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}

fn resume_from_checkpoint<'a>(
    checkpoint_path: &str,
    all_files: &'a mut HashSet<FileItem>,
    export_data: &Arc<Mutex<Vec<ExportFrame>>>,
) -> Result<&'a mut HashSet<FileItem>> {
    let checkpoint = Path::new(checkpoint_path);
    if !checkpoint.exists() {
        log::error!("Checkpoint file does not exist");
        return Err(anyhow::anyhow!("Checkpoint file does not exist"));
    }
    if !checkpoint.is_file() {
        log::error!("Checkpoint path is not a file");
        return Err(anyhow::anyhow!("Checkpoint path is not a file"));
    }
    match checkpoint.extension() {
        Some(ext) => {
            let ext = ext.to_str().unwrap();
            if ext != "json" && ext != "csv" {
                log::error!("Invalid checkpoint file extension: {}", ext);
                return Err(anyhow::anyhow!(
                    "Invalid checkpoint file extension: {}",
                    ext
                ));
            } else {
                let frames;
                if ext == "json" {
                    let json = std::fs::read_to_string(checkpoint)?;
                    frames = serde_json::from_str(&json)?;
                } else {
                    frames = parse_export_csv(checkpoint)?;
                }
                let mut file_frame_count = HashMap::new();
                let mut file_total_frames = HashMap::new();
                let mut file_has_error = HashMap::new();
                for f in &frames {
                    let file = &f.file;
                    if f.error.is_some() {
                        file_has_error.insert(file.clone(), true);
                        continue;
                    }
                    let count = file_frame_count.entry(file.clone()).or_insert(0);
                    *count += 1;
                    file_total_frames
                        .entry(file.clone())
                        .or_insert(f.total_frames);
                }
                for (file, total_frames) in file_total_frames.iter() {
                    let frame_count = file_frame_count.get(file).copied().unwrap_or(0);
                    let has_error = file_has_error.get(file).copied().unwrap_or(false);
                    if !has_error && *total_frames == frame_count {
                        all_files.remove(file);
                    }
                }
                export_data.lock().unwrap().extend_from_slice(&frames);
                Ok(all_files)
            }
        }
        None => {
            log::error!("Invalid checkpoint file extension");
            return Err(anyhow::anyhow!("Invalid checkpoint file extension"));
        }
    }
}

#[tauri::command]
async fn check_health(app: AppHandle, grpc_url: String) {
    match get_health(grpc_url).await {
        Ok(health) => {
            app.emit("health-status", health).unwrap();
        }
        Err(err) => {
            // Log the error
            log::error!("Health check failed: {}", err);

            app.emit("health-status", false).unwrap();
        }
    }
}

#[tauri::command]
async fn check_quota(app: AppHandle, grpc_url: String, token: String) {
    if let Ok(quota) = get_auth(grpc_url, token).await {
        app.emit("quota", quota).unwrap();
    } else {
        app.emit("quota", None::<i32>).unwrap();
    }
}

#[tauri::command]
async fn check_path_exists(path_str: String) -> Result<bool, String> {
    let path = std::path::PathBuf::from(path_str);
    Ok(path.exists())
}

#[tauri::command]
async fn process_media(app: AppHandle, config: Config) {
    let (progress_sender, progress_receiver) = crossbeam_channel::bounded(5);

    let total_files;

    match crate::utils::index_files_and_folders(&PathBuf::from(
        &config.detect_options.selected_folder,
    )) {
        Ok(files) => {
            total_files = files.len();
        }
        Err(e) => {
            log::error!("{}", e);
            app.emit("detect-error", e.to_string()).unwrap();
            return;
        }
    }

    let app_clone = app.clone();

    let progress_thread = std::thread::spawn(move || {
        let mut progress = 0.0;
        for _ in progress_receiver.iter() {
            progress += 1.0 / total_files as f32 * 100.0;
            app_clone
                .emit("detect-progress", progress)
                .unwrap();
        }
    });

    match process(config, progress_sender).await {
        Ok(_) => {
            app.emit("detect-complete", 1).unwrap();
        }
        Err(e) => {
            app.emit("detect-error", e.to_string()).unwrap();
            log::error!("Error processing: {}", e);
        }
    }
    progress_thread.join().unwrap();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .filter(|metadata| metadata.target() != "hyper")
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            process_media,
            check_health,
            check_quota,
            check_path_exists,
        ])
        .setup(|app| {
            let _ = app.store("store.json")?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
