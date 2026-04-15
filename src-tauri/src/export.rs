use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use csv::WriterBuilder;
use serde::{Deserialize, Serialize};

use crate::utils::FileItem;
use crate::ExportFormat;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Bbox {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub score: f32,
    pub class: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportFrame {
    #[serde(flatten)]
    pub file: FileItem,
    pub shoot_time: Option<String>,
    pub frame_index: usize,
    pub total_frames: usize,
    pub bboxes: Option<Vec<Bbox>>,
    pub label: Option<Vec<String>>,
    pub error: Option<String>,
    pub iframe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSummary {
    pub total_files: usize,
    pub skipped_files: usize,
    pub processed_files: usize,
    pub success_files: usize,
    pub error_files: usize,
    pub result_path: String,
    pub error_report_path: Option<String>,
}

pub fn parse_export_csv<P: AsRef<Path>>(csv: P) -> Result<Vec<ExportFrame>> {
    let file = File::open(csv)?;
    let mut rdr = csv::Reader::from_reader(file);
    let mut export_data = Vec::new();
    for frame in rdr.records() {
        let frame = frame?;
        let file_item = FileItem {
            folder_id: frame[0].parse::<_>()?,
            file_id: frame[1].parse::<_>()?,
            file_path: frame[2].parse()?,
            tmp_path: frame[2].parse()?,
        };
        let bboxes = frame[7].to_string().replace("\"\"", "\"");
        let bboxes = serde_json::from_str(&bboxes)?;
        let frame_item = ExportFrame {
            file: file_item,
            shoot_time: if frame[3].is_empty() {
                None
            } else {
                Some(frame[3].to_string())
            },
            frame_index: frame[4].parse::<_>()?,
            total_frames: frame[5].parse::<_>()?,
            bboxes,
            label: if frame[8].is_empty() {
                None
            } else {
                Some(
                    frame[8]
                    .to_string()
                    .split(";")
                    .map(|s| s.to_string())
                    .collect(),
                )
            },
            iframe: frame[6].parse::<_>()?,
            error: if frame[9].is_empty() {
                None
            } else {
                Some(frame[9].to_string())
            },
        };
        export_data.push(frame_item);
    }
    Ok(export_data)
}

pub fn export_worker(
    checkpoint: usize,
    checkpoint_counter: &Arc<Mutex<usize>>,
    format: &ExportFormat,
    folder_path: &PathBuf,
    export_q_r: crossbeam_channel::Receiver<ExportFrame>,
    export_data: &Arc<Mutex<Vec<ExportFrame>>>,
) {
    let mut updated_files = HashSet::new();
    loop {
        match export_q_r.recv() {
            Ok(export_frame) => {
                let mut export_data = export_data.lock().unwrap();
                replace_file_rows(&mut export_data, &export_frame, &mut updated_files);
                let mut checkpoint_counter = checkpoint_counter.lock().unwrap();
                if *checkpoint_counter % checkpoint == 0 && *checkpoint_counter != 0 {
                    log::info!("Exported {} frames", export_data.len());
                    match format {
                        ExportFormat::Json => write_json(&export_data, folder_path).unwrap(),
                        ExportFormat::Csv => write_csv(&export_data, folder_path).unwrap(),
                    }
                }
                export_data.push(export_frame);
                *checkpoint_counter += 1;
            }
            Err(_) => break,
        }
    }
}

fn replace_file_rows(
    export_data: &mut Vec<ExportFrame>,
    export_frame: &ExportFrame,
    updated_files: &mut HashSet<PathBuf>,
) {
    if updated_files.insert(export_frame.file.file_path.clone()) {
        export_data.retain(|frame| frame.file.file_path != export_frame.file.file_path);
    }
}

fn write_json<P: AsRef<Path>>(export_data: &[ExportFrame], path: P) -> Result<()> {
    let json = serde_json::to_string_pretty(export_data)?;
    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}

fn write_csv<P: AsRef<Path>>(export_data: &[ExportFrame], path: P) -> Result<()> {
    let mut wtr = WriterBuilder::new()
        .has_headers(false)
        .from_path(path)?;
    wtr.write_record([
        "folder_id",
        "file_id",
        "file_path",
        "shoot_time",
        "frame_index",
        "total_frames",
        "bboxes",
        "label",
        "error",
    ])?;
    for export_frame in export_data {
        wtr.write_record(&[
            export_frame.file.folder_id.to_string().as_str(),
            export_frame.file.file_id.to_string().as_str(),
            export_frame
                .file
                .file_path
                .to_string_lossy()
                .into_owned()
                .as_str(),
            export_frame
                .shoot_time
                .clone()
                .unwrap_or("".to_string())
                .as_str(),
            export_frame.frame_index.to_string().as_str(),
            export_frame.total_frames.to_string().as_str(),
            serde_json::to_string(&export_frame.bboxes)
                .unwrap_or("".to_string())
                .as_str(),
            &itertools::join(
                export_frame.label.clone().unwrap_or(vec!["".to_string()]),
                ";",
            ),
            export_frame
                .error
                .clone()
                .unwrap_or("".to_string())
                .as_str(),
        ])?;
    }
    wtr.flush()?;
    Ok(())
}

pub fn export(
    folder_path: &PathBuf,
    export_data: Arc<Mutex<Vec<ExportFrame>>>,
    export_format: &ExportFormat,
) -> Result<ProcessSummary> {
    let export_data = export_data.lock().unwrap();
    log::info!("Exported {} frames", export_data.len());
    let result_path = result_path(folder_path, export_format);
    match export_format {
        ExportFormat::Json => {
            write_json(&export_data, &result_path)?;
        }
        ExportFormat::Csv => {
            write_csv(&export_data, &result_path)?;
        }
    }
    Ok(ProcessSummary {
        total_files: 0,
        skipped_files: 0,
        processed_files: 0,
        success_files: 0,
        error_files: 0,
        result_path: result_path.to_string_lossy().into_owned(),
        error_report_path: None,
    })
}

pub fn finalize_run(
    folder_path: &PathBuf,
    export_data: Arc<Mutex<Vec<ExportFrame>>>,
    export_format: &ExportFormat,
    run_files: &HashSet<PathBuf>,
    total_files: usize,
    skipped_files: usize,
) -> Result<ProcessSummary> {
    let mut summary = export(folder_path, export_data, export_format)?;
    summary.total_files = total_files;
    summary.skipped_files = skipped_files;
    summary.processed_files = run_files.len();

    let export_data = std::fs::read_to_string(&summary.result_path)?;
    let export_data: Vec<ExportFrame> = match export_format {
        ExportFormat::Json => serde_json::from_str(&export_data)?,
        ExportFormat::Csv => parse_export_csv(&summary.result_path)?,
    };

    let mut file_has_error = std::collections::HashMap::new();
    let mut error_rows = Vec::new();
    for frame in export_data.iter() {
        if !run_files.contains(&frame.file.file_path) {
            continue;
        }
        let has_error = frame.error.is_some();
        file_has_error
            .entry(frame.file.file_path.clone())
            .and_modify(|current| *current |= has_error)
            .or_insert(has_error);
        if has_error {
            error_rows.push(frame.clone());
        }
    }

    summary.error_files = file_has_error.values().filter(|has_error| **has_error).count();
    summary.success_files = summary
        .processed_files
        .saturating_sub(summary.error_files);
    let error_report_path = error_report_path(folder_path, export_format);
    if error_rows.is_empty() {
        if error_report_path.exists() {
            std::fs::remove_file(&error_report_path)?;
        }
        summary.error_report_path = None;
    } else {
        match export_format {
            ExportFormat::Json => write_json(&error_rows, &error_report_path)?,
            ExportFormat::Csv => write_csv(&error_rows, &error_report_path)?,
        }
        summary.error_report_path = Some(error_report_path.to_string_lossy().into_owned());
    }

    Ok(summary)
}

fn result_path(folder_path: &Path, export_format: &ExportFormat) -> PathBuf {
    match export_format {
        ExportFormat::Json => folder_path.join("result.json"),
        ExportFormat::Csv => folder_path.join("result.csv"),
    }
}

fn error_report_path(folder_path: &Path, export_format: &ExportFormat) -> PathBuf {
    match export_format {
        ExportFormat::Json => folder_path.join("result.errors.json"),
        ExportFormat::Csv => folder_path.join("result.errors.csv"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_export_csv() {
        // let csv = Path::new("input/result.csv");
        let export_data = parse_export_csv("input/result.csv").unwrap();
        assert_eq!(export_data.len(), 11);
    }
}
