// scripts/download-ffmpeg.js
import path from "path";
import https from "https";
import { execSync } from "child_process";
import {
    createWriteStream,
    unlinkSync,
    statSync,
    readdirSync,
    copyFileSync,
} from "fs";
import { fileURLToPath } from "url";
import fs from "fs";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const binaryDir = path.join(__dirname, "..", "src-tauri", "binaries");
const ORGANIZE_RELEASE_BASE_URL =
    "https://github.com/wsyxbcl/organize/releases/download/v0.1.1%2Bmazefork.1";

const rustInfo = execSync("rustc -vV");
const targetTriple = /host: (\S+)/g.exec(rustInfo)[1];
const extension = targetTriple === "x86_64-pc-windows-msvc" ? ".exe" : "";

const ffmpegBinary = path.join(binaryDir, `ffmpeg-${targetTriple}${extension}`);
const ffprobeBinary = path.join(
    binaryDir,
    `ffprobe-${targetTriple}${extension}`
);
const organizeBinary = path.join(
    binaryDir,
    `organize-${targetTriple}${extension}`
);

if (!targetTriple) {
    console.error("Failed to determine platform target triple");
}

// Create directory if it doesn't exist
if (!fs.existsSync(binaryDir)) {
    fs.mkdirSync(binaryDir, { recursive: true });
}

function getFFmpegInfo() {
    const rustInfo = execSync("rustc -vV");
    const targetTriple = /host: (\S+)/g.exec(rustInfo)[1];

    if (targetTriple === "x86_64-pc-windows-msvc") {
        return {
            url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-win64-lgpl-7.1.zip",
            outputPath: path.join(
                binaryDir,
                "ffmpeg-x86_64-pc-windows-msvc.zip"
            ),
            extractDir: binaryDir,
        };
    } else if (targetTriple === "aarch64-apple-darwin") {
        return {
            url: "https://github.com/simulacraliasing/ffmpeg-macos-build/releases/download/v7.1/ffmpeg71arm.zip",
            outputPath: path.join(binaryDir, "ffmpeg-aarch64-apple-darwin.zip"),
            extractDir: binaryDir,
        };
    } else if (targetTriple === "x86_64-apple-darwin") {
        return {
            url: "https://github.com/simulacraliasing/ffmpeg-macos-build/releases/download/v7.1/ffmpeg71intel.zip",
            outputPath: path.join(binaryDir, "ffmpeg-x86_64-apple-darwin.zip"),
            extractDir: binaryDir,
        };
    } else if (targetTriple === "x86_64-unknown-linux-gnu") {
        return {
            url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-linux64-lgpl-7.1.tar.xz",
            outputPath: path.join(
                binaryDir,
                "ffmpeg-x86_64-unknown-linux-gnu.tar.xz"
            ),
            extractDir: binaryDir,
        };
    } else if (targetTriple === "aarch64-unknown-linux-gnu") {
        return {
            url: "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-linuxarm64-lgpl-7.1.tar.xz",
            outputPath: path.join(
                binaryDir,
                "ffmpeg-aarch64-unknown-linux-gnu.tar.xz"
            ),
            extractDir: binaryDir,
        };
    } else {
        throw new Error(`Unsupported target triple: ${targetTriple}`);
    }
}

function getOrganizeInfo() {
    const rustInfo = execSync("rustc -vV");
    const targetTriple = /host: (\S+)/g.exec(rustInfo)[1];

    if (targetTriple === "x86_64-pc-windows-msvc") {
        return {
            url: `${ORGANIZE_RELEASE_BASE_URL}/organize-x86_64-pc-windows-msvc.exe`,
            outputPath: path.join(
                binaryDir,
                "organize-x86_64-pc-windows-msvc.exe"
            ),
        };
    } else if (targetTriple === "aarch64-apple-darwin") {
        return {
            url: `${ORGANIZE_RELEASE_BASE_URL}/organize-aarch64-apple-darwin`,
            outputPath: path.join(binaryDir, "organize-aarch64-apple-darwin"),
        };
    } else if (targetTriple === "x86_64-apple-darwin") {
        return {
            url: `${ORGANIZE_RELEASE_BASE_URL}/organize-x86_64-apple-darwin`,
            outputPath: path.join(binaryDir, "organize-x86_64-apple-darwin"),
        };
    } else if (targetTriple === "x86_64-unknown-linux-gnu") {
        return {
            url: `${ORGANIZE_RELEASE_BASE_URL}/organize-x86_64-unknown-linux-gnu`,
            outputPath: path.join(
                binaryDir,
                "organize-x86_64-unknown-linux-gnu"
            ),
        };
    } else if (targetTriple === "aarch64-unknown-linux-gnu") {
        return {
            url: `${ORGANIZE_RELEASE_BASE_URL}/organize-aarch64-unknown-linux-gnu`,
            outputPath: path.join(
                binaryDir,
                "organize-aarch64-unknown-linux-gnu"
            ),
        };
    } else {
        throw new Error(`Unsupported target triple: ${targetTriple}`);
    }
}

// Download file from URL
async function downloadFile(fileUrl, outputPath) {
    return new Promise((resolve, reject) => {
        const file = createWriteStream(outputPath);

        const handleRedirect = (response) => {
            if (
                response.statusCode >= 300 &&
                response.statusCode < 400 &&
                response.headers.location
            ) {
                const newUrl = new URL(
                    response.headers.location,
                    fileUrl
                ).toString();
                https.get(newUrl, handleRedirect).on("error", reject);
            } else if (response.statusCode !== 200) {
                reject(new Error(`Failed to download: ${response.statusCode}`));
            } else {
                response.pipe(file);
                file.on("finish", () => {
                    file.close();
                    resolve();
                });
            }
        };

        https.get(fileUrl, handleRedirect).on("error", (err) => {
            fs.unlink(outputPath, () => {});
            reject(err);
        });
    });
}

// Extract the downloaded file
async function extractFile(filePath, extractDir, targetTriple) {
    console.log(`Extracting to ${extractDir}...`);

    if (targetTriple === "x86_64-pc-windows-msvc") {
        // For Windows, we need to use a library to extract zip files
        // You can use a library like 'extract-zip' or 'unzipper'
        // For simplicity, we'll use 7zip if available, or fallback to PowerShell
        try {
            execSync(
                `powershell -command "Expand-Archive -Path '${filePath}' -DestinationPath '${extractDir}' -Force"`
            );

            // Find the ffmpeg.exe in the extracted directory
            const ffmpegExe = findFileRecursive(extractDir, "ffmpeg.exe");
            if (ffmpegExe) {
                // Move ffmpeg.exe to the root of the windows directory
                copyFileSync(
                    ffmpegExe,
                    path.join(binaryDir, `ffmpeg-${targetTriple}.exe`)
                );
                console.log("Copied ffmpeg.exe to windows directory");
            } else {
                throw new Error("Could not find ffmpeg.exe in extracted files");
            }

            const ffprobeExe = findFileRecursive(extractDir, "ffprobe.exe");
            if (ffprobeExe) {
                copyFileSync(
                    ffprobeExe,
                    path.join(binaryDir, `ffprobe-${targetTriple}.exe`)
                );
                console.log("Copied ffprobe.exe to windows directory");
            } else {
                throw new Error(
                    "Could not find ffprobe.exe in extracted files"
                );
            }
        } catch (err) {
            console.error("Error extracting with PowerShell:", err);
            throw err;
        }
    } else {
        try {
            // Extract the tarball
            if (filePath.endsWith(".tar.xz")) {
                execSync(`tar -xf "${filePath}" -C "${extractDir}"`);
            } else if (filePath.endsWith(".zip")) {
                execSync(`unzip -o "${filePath}" -d "${extractDir}"`);
            } else {
                throw new Error("Unsupported file extension");
            }

            // macOS typically has the ffmpeg binary directly in the zip
            const ffmpegBin = findFileRecursive(extractDir, "ffmpeg");
            if (ffmpegBin) {
                let destPath = path.join(binaryDir, `ffmpeg-${targetTriple}`);
                copyFileSync(ffmpegBin, destPath);
                console.log("Copied ffmpeg to macos directory");

                // Make executable
                execSync(`chmod +x ${destPath}`);
            } else {
                throw new Error(
                    "Could not find ffmpeg binary in extracted files"
                );
            }
            const ffprobeBin = findFileRecursive(extractDir, "ffprobe");
            if (ffprobeBin) {
                let destPath = path.join(binaryDir, `ffprobe-${targetTriple}`);
                copyFileSync(ffprobeBin, destPath);
                console.log(`Copied ffprobe to ${targetTriple} directory`);

                // Make executable on non-Windows platforms
                execSync(`chmod +x ${destPath}`);
            } else {
                throw new Error(
                    "Could not find ffprobe binary in extracted files"
                );
            }
        } catch (err) {
            console.error("Error extracting with unzip:", err);
            throw err;
        }
    }
}

// Helper function to find a file recursively
function findFileRecursive(dir, filename) {
    const files = readdirSync(dir);

    for (const file of files) {
        const filePath = path.join(dir, file);
        const stat = statSync(filePath);

        if (stat.isDirectory()) {
            const found = findFileRecursive(filePath, filename);
            if (found) return found;
        } else if (file === filename) {
            return filePath;
        }
    }

    return null;
}

// Clean up temporary files
function cleanUp(filePath) {
    try {
        unlinkSync(filePath);
        console.log(`Cleaned up temporary file: ${filePath}`);
    } catch (err) {
        console.error(`Failed to clean up ${filePath}:`, err);
    }
}

// Download and extract FFmpeg
async function downloadFFmpeg() {
    if (fs.existsSync(ffmpegBinary) && fs.existsSync(ffprobeBinary)) {
        console.log("FFmpeg already exists, skipping download");
        return;
    }

    const { url, outputPath, extractDir } = getFFmpegInfo();

    try {
        console.log(`Downloading FFmpeg for ${targetTriple} from ${url}...`);
        await downloadFile(url, outputPath);
        console.log("Download complete!");

        await extractFile(outputPath, extractDir, targetTriple);
        console.log("Extraction complete!");

        // Clean up the downloaded archive
        cleanUp(outputPath);

        console.log(
            "FFmpeg has been successfully installed for your platform!"
        );
    } catch (error) {
        console.error("Error downloading or extracting FFmpeg:", error);
        process.exit(1);
    }
}

async function downloadOrganize() {
    if (fs.existsSync(organizeBinary)) {
        console.log("Organize already exists, skipping download");
        return;
    }

    const { url, outputPath } = getOrganizeInfo();

    try {
        console.log(`Downloading Organize for ${targetTriple} from ${url}...`);
        await downloadFile(url, outputPath);
        console.log("Download complete!");

        if (targetTriple !== "x86_64-pc-windows-msvc") {
            // Make the downloaded file executable
            execSync(`chmod +x ${outputPath}`);
        }

        console.log(
            "Organize has been successfully installed for your platform!"
        );
    } catch (error) {
        console.error("Error downloading Organize:", error);
        process.exit(1);
    }
}

// Run the download process
downloadFFmpeg();

downloadOrganize();
