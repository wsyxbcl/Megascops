use std::fs::{metadata, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, TimeZone};
use crossbeam_channel::Sender;
use fast_image_resize::{ResizeAlg, ResizeOptions, Resizer};
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel};
use ffmpeg_sidecar::ffprobe::ffprobe_path;
use ffmpeg_sidecar::iter::FfmpegIterator;
use image::{DynamicImage, GenericImageView, ImageReader};
use jpeg_decoder::Decoder;
use nom_exif::{EntryValue, Exif, ExifIter, ExifTag, MediaParser, MediaSource};
use thiserror::Error;
use webp::Encoder;

use crate::ExportFrame;
use crate::utils::{sample_evenly, FileItem};

//define meadia error
#[derive(Error, Debug)]
pub enum MediaError {
    #[error("Failed to open file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to decode: {0}")]
    ImageDecodeError(#[from] jpeg_decoder::Error),

    #[error("Failed to decode: {0}")]
    VideoDecodeError(String),

    #[error("Failed to encode: {0}")]
    WebpEncodeError(String),

    #[error("Ffmpeg error when decoding {1}: {0}")]
    FfmpegError(String, String),
}

pub struct Frame {
    pub file: FileItem,
    pub webp: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub frame_index: usize,
    pub total_frames: usize,
    pub shoot_time: Option<DateTime<Local>>,
    pub iframe: bool,
}

pub struct ErrFile {
    pub file: FileItem,
    pub error: anyhow::Error,
}

pub enum WebpItem {
    Frame(Frame),
    ErrFile(ErrFile),
}

pub fn media_worker(
    file: FileItem,
    imgsz: usize,
    quality: f32,
    iframe: bool,
    max_frames: Option<usize>,
    array_q_s: Sender<WebpItem>,
    export_q_s: Sender<ExportFrame>,
    progress_sender: Sender<usize>,
    cancel_flag: Arc<AtomicBool>,
) {
    if cancel_flag.load(Ordering::Relaxed) {
        if &file.file_path != &file.tmp_path {
            let _ = remove_file_with_retries(&file.tmp_path, 3, Duration::from_secs(1));
        }
        return;
    }
    let mut parser = MediaParser::new();
    let mut resizer = Resizer::new();
    if let Some(extension) = file.file_path.extension() {
        let array_q_s = array_q_s.clone();
        match extension.to_str().unwrap().to_lowercase().as_str() {
            "jpg" | "jpeg" | "png" => {
                if let Err(e) =
                    process_image(&file, imgsz, quality, &mut parser, &mut resizer, array_q_s)
                {
                    log::error!(
                        "Failed to process image {}: {}",
                        file.file_path.display(),
                        e
                    );
                    send_export_error(&export_q_s, &file, &e);
                    cancel_flag.store(true, Ordering::Relaxed);
                    if &file.file_path != &file.tmp_path {
                        let _ = remove_file_with_retries(&file.tmp_path, 3, Duration::from_secs(1));
                    }
                    return;
                }
            }
            "mp4" | "avi" | "mkv" | "mov" => {
                if let Err(e) =
                    process_video(&file, imgsz, quality, iframe, max_frames, array_q_s)
                {
                    log::error!(
                        "Failed to process video {}: {}",
                        file.file_path.display(),
                        e
                    );
                    send_export_error(&export_q_s, &file, &e);
                    cancel_flag.store(true, Ordering::Relaxed);
                    if &file.file_path != &file.tmp_path {
                        let _ = remove_file_with_retries(&file.tmp_path, 3, Duration::from_secs(1));
                    }
                    return;
                }
            }
            _ => (),
        }
        if &file.file_path != &file.tmp_path {
            remove_file_with_retries(&file.tmp_path, 3, Duration::from_secs(1))
                .expect("Failed to remove file");
        }
        progress_sender.send(1).expect("Send progress failed");
    }
}

fn remove_file_with_retries(file_path: &PathBuf, max_retries: u32, delay: Duration) -> Result<()> {
    let mut attempts = 0;

    while attempts < max_retries {
        match std::fs::remove_file(file_path) {
            Ok(_) => {
                log::debug!("File removed successfully.");
                return Ok(());
            }
            Err(e) => {
                log::error!(
                    "Failed to remove file: {}. Attempt {} of {}",
                    e,
                    attempts + 1,
                    max_retries
                );
                attempts += 1;

                if attempts < max_retries {
                    thread::sleep(delay);
                }
            }
        }
    }

    Ok(())
}

fn send_export_error(export_q_s: &Sender<ExportFrame>, file: &FileItem, error: &anyhow::Error) {
    let export_frame = ExportFrame {
        file: file.clone(),
        shoot_time: None,
        frame_index: 0,
        total_frames: 0,
        bboxes: None,
        label: None,
        error: Some(error.to_string()),
        iframe: false,
    };
    if let Err(send_err) = export_q_s.send(export_frame) {
        log::error!(
            "Failed to write error result for {}: {}",
            file.file_path.display(),
            send_err
        );
    }
}

fn decode_image(file: &FileItem) -> Result<DynamicImage> {
    let img = match ImageReader::open(file.tmp_path.as_path())
        .map_err(MediaError::IoError)?
        .decode()
    {
        Ok(img) => DynamicImage::ImageRgb8(img.to_rgb8()),
        Err(_e) => {
            log::warn!(
                "Failed to decode image with ImageReader. Trying jpeg_decoder. {:?}",
                _e
            );
            let img_reader = File::open(file.tmp_path.as_path()).map_err(MediaError::IoError)?;
            let mut decoder = Decoder::new(BufReader::new(img_reader));
            let pixels = decoder.decode().map_err(MediaError::ImageDecodeError)?;
            let img = DynamicImage::ImageRgb8(
                image::ImageBuffer::from_raw(
                    decoder.info().unwrap().width as u32,
                    decoder.info().unwrap().height as u32,
                    pixels,
                )
                .unwrap(),
            );
            img
        }
    };
    Ok(img)
}

pub fn process_image(
    file: &FileItem,
    imgsz: usize,
    quality: f32,
    parser: &mut MediaParser,
    resizer: &mut Resizer,
    array_q_s: Sender<WebpItem>,
) -> Result<()> {
    let frame_data = match decode_image(file) {
        Ok(img) => {
            let webp: Option<Vec<u8>> = match resize_encode(&img, imgsz as u32, quality, resizer) {
                Ok(webp) => Some(webp),
                Err(_e) => None,
            };
            let shoot_time: Option<DateTime<Local>> =
                match get_image_date(parser, file.tmp_path.as_path()) {
                    Ok(shoot_time) => Some(shoot_time),
                    Err(_e) => {
                        log::error!(
                            "Failed to get {} shoot time error: {}",
                            file.file_path.display(),
                            _e
                        );
                        None
                    }
                };
            if webp.is_none() {
                WebpItem::ErrFile(ErrFile {
                    file: file.clone(),
                    error: MediaError::WebpEncodeError("Failed to encode image".to_string()).into(),
                })
            } else {
                let webp = webp.unwrap();
                let frame_data = Frame {
                    webp,
                    file: file.clone(),
                    width: img.width() as usize,
                    height: img.height() as usize,
                    frame_index: 0,
                    total_frames: 1,
                    shoot_time,
                    iframe: false,
                };
                WebpItem::Frame(frame_data)
            }
        }
        Err(error) => WebpItem::ErrFile(ErrFile {
            file: file.clone(),
            error,
        }),
    };
    array_q_s
        .send(frame_data)
        .map_err(|e| anyhow::anyhow!(
            "Failed to send frame data for {}: {}",
            file.file_path.display(),
            e
        ))?;
    Ok(())
}

fn resize_encode(
    img: &DynamicImage,
    imgsz: u32,
    quality: f32,
    resizer: &mut Resizer,
) -> Result<Vec<u8>> {
    // Get the dimensions of the original image
    let (width, height) = img.dimensions();
    let mut resized_width = imgsz;
    let mut resized_height = imgsz;
    let ratio: f32;

    if width > height {
        ratio = width as f32 / imgsz as f32;
        resized_height = (height as f32 / ratio) as u32;
        resized_height = resized_height % 2 + resized_height;
    } else {
        ratio = height as f32 / imgsz as f32;
        resized_width = (width as f32 / ratio) as u32;
        resized_width = resized_width % 2 + resized_width;
    }

    let mut resized_img = DynamicImage::new(resized_width, resized_height, img.color());

    let resize_option = ResizeOptions::new().resize_alg(ResizeAlg::Nearest);

    resizer
        .resize(img, &mut resized_img, &resize_option)
        .unwrap();

    let encoder = Encoder::from_image(&resized_img);

    match encoder {
        Ok(encoder) => {
            let webp = encoder.encode(quality);
            let data = (&*webp).to_vec();
            Ok(data)
        }
        Err(e) => {
            log::error!("Failed to encode image: {:?}", e);
            Err(MediaError::WebpEncodeError(e.to_string()).into())
        }
    }
}

pub fn process_video(
    file: &FileItem,
    imgsz: usize,
    quality: f32,
    iframe: bool,
    max_frames: Option<usize>,
    array_q_s: Sender<WebpItem>,
) -> Result<()> {
    let video_path = file.tmp_path.to_string_lossy();
    let (orig_w, orig_h) = match get_video_dimensions(&video_path) {
        Ok(dim) => dim,
        Err(e) => {
            let error = anyhow!(e).context("Failed to get video dimensions");
            log::error!("{}", error);
            let err_file = WebpItem::ErrFile(ErrFile {
                file: file.clone(),
                error,
            });
            array_q_s
                .send(err_file)
                .context("Failed to send dimension error")?;
            return Ok(());
        }
    };
    let input = match create_ffmpeg_iter(&video_path, imgsz, iframe) {
        Ok(input) => input,
        Err(error) => {
            log::error!("Failed to create ffmpeg iterator: {}", error);
            let err_file = WebpItem::ErrFile(ErrFile {
                file: file.clone(),
                error: error.context("Failed to create ffmpeg iterator"),
            });
            array_q_s
                .send(err_file)
                .context("Failed to send ffmpeg iterator error")?;
            return Ok(());
        }
    };

    handle_ffmpeg_output(
        input, array_q_s, file, quality, max_frames, orig_w, orig_h, iframe,
    )?;

    Ok(())
}

fn get_video_dimensions(video_path: &str) -> Result<(usize, usize)> {
    let mut command = Command::new(ffprobe_path());

    command.args([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height",
        "-of",
        "csv=s=x:p=0",
        video_path,
    ]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let dimensions = str::from_utf8(&output.stdout)?;
    let parts: Vec<&str> = dimensions.trim().split('x').collect();

    if parts.len() == 2 {
        let width = parts[0].parse::<usize>()?;
        let height = parts[1].parse::<usize>()?;
        Ok((width, height))
    } else {
        Err(anyhow!(
            "Invalid video dimensions: {}, video path: {}",
            dimensions,
            video_path
        ))
    }
}

fn create_ffmpeg_iter(video_path: &str, imgsz: usize, iframe: bool) -> Result<FfmpegIterator> {
    let mut ffmpeg_command = FfmpegCommand::new();
    if iframe {
        ffmpeg_command.args(["-skip_frame", "nokey"]);
    }
    let iter = ffmpeg_command
        .input(video_path)
        .args(&[
            "-an",
            "-vf",
            &format!(
                "scale=w={}:h={}:force_original_aspect_ratio=decrease",
                imgsz, imgsz
            ),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-vsync",
            "vfr",
        ])
        .output("-")
        .spawn()?
        .iter()?;
    Ok(iter)
}

fn handle_ffmpeg_output(
    input: FfmpegIterator,
    s: Sender<WebpItem>,
    file: &FileItem,
    quality: f32,
    max_frames: Option<usize>,
    orig_w: usize,
    orig_h: usize,
    iframe: bool,
) -> Result<()> {
    let file_path = file.file_path.to_string_lossy().into_owned();

    let mut frames = Vec::new();
    let mut ffmpeg_error = Vec::new();
    for event in input {
        match event {
            FfmpegEvent::Error(e) | FfmpegEvent::Log(LogLevel::Error, e) => {
                ffmpeg_error.push(e);
            }
            FfmpegEvent::OutputFrame(frame) => {
                frames.push(frame);
            }
            _ => (),
        }
    }

    for e in ffmpeg_error {
        let error = MediaError::FfmpegError(e, file_path.clone());
        log::warn!("{:?}", error);
    }

    if frames.is_empty() {
        let error = MediaError::VideoDecodeError(file_path).into();
        log::error!("{:?}", error);
        let frame_data = WebpItem::ErrFile(ErrFile {
            file: file.clone(),
            error,
        });
        if let Err(e) = s.send(frame_data) {
            let err_msg = format!(
                "Failed to send ffmpeg error frame for {}: {}",
                file.file_path.display(),
                e
            );
            log::error!("{}", err_msg);
            return Err(anyhow!(err_msg));
        }
    } else {
        let sampled_frames = sample_evenly(&frames, max_frames.unwrap_or(frames.len()));

        let shoot_time: Option<DateTime<Local>> = match get_video_date(&file.tmp_path.as_path()) {
            Ok(shoot_time) => Some(shoot_time),
            Err(_e) => None,
        };

        //calculate ratio and padding

        let frames_length = sampled_frames.len();

        for f in sampled_frames.into_iter() {
            let encoder = Encoder::from_rgb(&f.data, f.width, f.height);

            let webp = encoder.encode(quality);

            let webp = (&*webp).to_vec();

            let frame_data = WebpItem::Frame(Frame {
                webp,
                file: file.clone(),
                width: orig_w,
                height: orig_h,
                frame_index: f.frame_num as usize,
                total_frames: frames_length,
                shoot_time,
                iframe,
            });
            if let Err(e) = s.send(frame_data) {
                let err_msg = format!(
                    "Failed to send frame data for {}: {}",
                    file.file_path.display(),
                    e
                );
                log::error!("{}", err_msg);
                return Err(anyhow!(err_msg));
            }
        }
    }
    Ok(())
}

fn get_image_date(parser: &mut MediaParser, image: &Path) -> Result<DateTime<Local>> {
    let ms = MediaSource::file_path(image)?;
    let iter: ExifIter = parser.parse(ms)?;
    let exif: Exif = iter.into();
    let shoot_time_tag = exif
        .get(ExifTag::DateTimeOriginal)
        .or_else(|| exif.get(ExifTag::ModifyDate))
        .context("Neither DateTimeOriginal nor ModifyDate found")?;

    let shoot_time = match shoot_time_tag {
        EntryValue::Time(time) => time.with_timezone(&Local),
        EntryValue::NaiveDateTime(time) => {
            Local.from_local_datetime(&time).single().ok_or_else(|| {
                anyhow::anyhow!("Ambiguous local time for image: {}", image.display())
            })?
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unexpected EXIF time data format for image: {}",
                image.display()
            ))
        }
    };
    Ok(shoot_time)
}

fn get_video_date(video: &Path) -> Result<DateTime<Local>> {
    let metadata = metadata(video)?;
    #[cfg(target_os = "windows")]
    {
        let m_time = metadata.modified()?;
        let shoot_time: DateTime<Local> = m_time.clone().into();

        Ok(shoot_time)
    }

    #[cfg(target_os = "linux")]
    #[allow(deprecated)]
    {
        use chrono::NaiveDateTime;
        use std::os::linux::fs::MetadataExt;
        let m_time: i64 = metadata.st_mtime();
        let c_time: i64 = metadata.st_ctime();
        let shoot_time = m_time.min(c_time);
        let offset = Local::now().offset().to_owned();
        let shoot_time = NaiveDateTime::from_timestamp(shoot_time, 0);
        let shoot_time = DateTime::<Local>::from_naive_utc_and_offset(shoot_time, offset);

        Ok(shoot_time)
    }

    #[cfg(target_os = "macos")]
    {
        use chrono::NaiveDateTime;
        use std::os::unix::fs::MetadataExt;
        let m_time: i64 = metadata.mtime();
        let c_time: i64 = metadata.ctime();
        let shoot_time = m_time.min(c_time);
        let offset = Local::now().offset().to_owned();
        let shoot_time = NaiveDateTime::from_timestamp(shoot_time, 0);
        let shoot_time = DateTime::<Local>::from_naive_utc_and_offset(shoot_time, offset);

        Ok(shoot_time)
    }
}
