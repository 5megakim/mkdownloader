use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const YT_DLP_SUMS_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
const GITHUB_LATEST_API: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";
#[cfg(windows)]
const FFMPEG_ZIP_URL: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download/ffmpeg-master-latest-win64-gpl.zip";
const APP_VERSION_URL: &str = "https://mkvibe.com/megabite/version.json";
const USER_AGENT: &str = "MK-YT-Downloader";

// ===== 플랫폼 분기 헬퍼 (Windows 동작은 기존 그대로, macOS 분기만 추가) =====

// version.json의 platforms 키 (Windows는 하위호환 위해 계속 app.* 를 읽음)
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
const PLATFORM_KEY: &str = "macos-aarch64";
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const PLATFORM_KEY: &str = "macos-x86_64";

// 도구 실행파일 이름: Windows는 .exe 붙음, macOS는 없음
fn tool_name(base: &str) -> String {
    #[cfg(windows)]
    {
        format!("{}.exe", base)
    }
    #[cfg(not(windows))]
    {
        base.to_string()
    }
}

// yt-dlp 공식 릴리스 자산 이름 (SHA2-256SUMS 행 이름과 동일)
fn yt_dlp_asset_name() -> &'static str {
    #[cfg(windows)]
    {
        "yt-dlp.exe"
    }
    #[cfg(target_os = "macos")]
    {
        "yt-dlp_macos"
    }
}

fn yt_dlp_download_url() -> String {
    format!(
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/{}",
        yt_dlp_asset_name()
    )
}

// macOS ffmpeg/ffprobe: OSXExperts 검증본을 우리 GitHub Releases에 미러한 고정 자산.
// zip / 추출 실행파일 이중 SHA256 고정 (2026-07-07 업스트림 게시값과 대조검증 + 미러 왕복검증 완료)
#[cfg(target_os = "macos")]
struct MacToolSource {
    url: &'static str,
    archive_sha256: &'static str,
    executable_sha256: &'static str,
    member: &'static str,
}

#[cfg(target_os = "macos")]
fn mac_ffmpeg_sources() -> [MacToolSource; 2] {
    #[cfg(target_arch = "aarch64")]
    {
        [
            MacToolSource {
                url: "https://github.com/5megakim/mkdownloader/releases/download/ffmpeg-macos-osxexperts-20260707/ffmpeg-8.1-macos-arm64-osxexperts.zip",
                archive_sha256: "ebb82529562b71170807bbc6b0e7eb4f0b13af8cbb0e085bb9e8f6fe709598ad",
                executable_sha256: "9a08d61f9328e8164ba560ee7a79958e357307fcfeea6fe626b7d66cdc287028",
                member: "ffmpeg",
            },
            MacToolSource {
                url: "https://github.com/5megakim/mkdownloader/releases/download/ffmpeg-macos-osxexperts-20260707/ffprobe-8.1-macos-arm64-osxexperts.zip",
                archive_sha256: "a6640a77d38a6f0527c5b597e599cb36a3427a6931444ed80bc62542421950a1",
                executable_sha256: "aab17ac7379c1178aaf400c3ef36cdb67db0b75b1a23eeef2cb9f658be8844e6",
                member: "ffprobe",
            },
        ]
    }
    #[cfg(target_arch = "x86_64")]
    {
        [
            MacToolSource {
                url: "https://github.com/5megakim/mkdownloader/releases/download/ffmpeg-macos-osxexperts-20260707/ffmpeg-8.0-macos-x86_64-osxexperts.zip",
                archive_sha256: "2d24d22db78c87f394a5822867acd5c5dc5e762cd261a44bd26923f3a5af3e07",
                executable_sha256: "df3f1e3facdc1ae0ad0bd898cdfb072fbc9641bf47b11f172844525a05db8d11",
                member: "ffmpeg",
            },
            MacToolSource {
                url: "https://github.com/5megakim/mkdownloader/releases/download/ffmpeg-macos-osxexperts-20260707/ffprobe-8.0-macos-x86_64-osxexperts.zip",
                archive_sha256: "0b6576104a95c1b39d4939e2df86f8f7cf1d55287ff57da48777d94605d12feb",
                executable_sha256: "5228e651e2bd67bb55819b27f6138351587b16d2b87446007bf35b7cf930d891",
                member: "ffprobe",
            },
        ]
    }
}

// macOS: 받은 바이너리 마무리 — 실행권한 + quarantine 제거 + ad-hoc 서명.
// 파일 내용이 바뀌므로(codesign) 반드시 SHA256 검증 이후, 최종 rename 이전(임시 파일)에 호출할 것.
// 실패는 숨기지 않고 에러로 반환한다 (깨진 파일이 "준비 완료"로 남는 것 방지).
#[cfg(target_os = "macos")]
fn finalize_mac_binary(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("실행권한 설정 실패: {}", e))?;

    // quarantine 속성이 애초에 없으면 xattr가 실패하는데 그것은 정상
    let x = std::process::Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(path)
        .output()
        .map_err(|e| format!("xattr 실행 실패: {}", e))?;
    if !x.status.success() {
        let err = String::from_utf8_lossy(&x.stderr).to_string();
        if !err.contains("No such xattr") {
            return Err(format!("quarantine 제거 실패: {}", err.trim()));
        }
    }

    let c = std::process::Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(path)
        .output()
        .map_err(|e| format!("codesign 실행 실패: {}", e))?;
    if !c.status.success() {
        return Err(format!(
            "ad-hoc 서명 실패: {}",
            String::from_utf8_lossy(&c.stderr).trim()
        ));
    }
    Ok(())
}

// Track the currently-running download process (yt-dlp PID) for cancellation
static CURRENT_DL_PID: OnceLock<Mutex<Option<u32>>> = OnceLock::new();

fn dl_pid_state() -> &'static Mutex<Option<u32>> {
    CURRENT_DL_PID.get_or_init(|| Mutex::new(None))
}

fn set_current_pid(pid: Option<u32>) {
    if let Ok(mut g) = dl_pid_state().lock() {
        *g = pid;
    }
}

// 프로세스와 그 자식들(yt-dlp가 띄우는 ffmpeg 등)을 통째로 종료.
// Windows = taskkill /F /T (기존 동작 유지), macOS = process group에 TERM → KILL.
// 이미 종료된 프로세스(No such process)는 성공으로 취급, 그 외 실패는 에러로 반환.
async fn terminate_process_tree(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/F", "/T", "/PID", &pid.to_string()]);
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
        cmd.spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        // make_command가 새 process group을 만들므로 그룹 전체(-pid) 대상
        let group = format!("-{}", pid);
        let term = std::process::Command::new("kill")
            .args(["-TERM", &group])
            .output()
            .map_err(|e| format!("kill 실행 실패: {}", e))?;
        if !term.status.success() {
            let err = String::from_utf8_lossy(&term.stderr).to_string();
            if !err.contains("No such process") {
                return Err(format!("취소 실패(TERM): {}", err.trim()));
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        let killo = std::process::Command::new("kill")
            .args(["-KILL", &group])
            .output()
            .map_err(|e| format!("kill 실행 실패: {}", e))?;
        if !killo.status.success() {
            let err = String::from_utf8_lossy(&killo.stderr).to_string();
            if !err.contains("No such process") {
                return Err(format!("취소 실패(KILL): {}", err.trim()));
            }
        }
        Ok(())
    }
}

#[tauri::command]
async fn cancel_download() -> Result<bool, String> {
    let pid = dl_pid_state().lock().ok().and_then(|g| *g);
    let Some(pid) = pid else {
        return Ok(false);
    };
    terminate_process_tree(pid).await?;
    set_current_pid(None);
    Ok(true)
}

fn binaries_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("앱 데이터 폴더 접근 실패: {}", e))?;
    Ok(base.join("binaries"))
}

#[derive(Serialize, Clone)]
struct VideoInfo {
    title: String,
    duration: u64,
    thumbnail: String,
    uploader: String,
    view_count: u64,
    max_height: u64,
    max_fps: f64,
    max_abr: f64,
    max_vcodec: String,
    max_acodec: String,
    is_live: bool,
    chapter_count: u64,
}

fn simplify_codec(c: &str) -> String {
    let c = c.to_lowercase();
    if c.starts_with("avc") || c.starts_with("h264") { "H.264".into() }
    else if c.starts_with("av01") { "AV1".into() }
    else if c.starts_with("vp9") { "VP9".into() }
    else if c.starts_with("vp8") { "VP8".into() }
    else if c.starts_with("hev") || c.starts_with("h265") { "H.265".into() }
    else if c.starts_with("opus") { "Opus".into() }
    else if c.starts_with("mp4a") || c.starts_with("aac") { "AAC".into() }
    else if c.starts_with("mp3") { "MP3".into() }
    else { c.split('.').next().unwrap_or(&c).to_uppercase() }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DownloadOptions {
    url: String,
    format_id: String,
    output_dir: String,
    subtitles_enabled: bool,
    subtitle_langs: String,
    subtitle_auto: bool,
    subtitle_embed: bool,
    sponsorblock: bool,
    rate_limit: String,
    cookies_browser: String,
    split_chapters: bool,
    download_section: String,
    live_from_start: bool,
    gif_fps: u32,
    gif_width: u32,
    duration_seconds: u64,
}

#[derive(Serialize, Clone, Default)]
struct DownloadProgress {
    percent: f64,
    speed: String,
    eta: String,
    total_size: String,
    status: String,
    message: String,
    #[serde(default)]
    current_part: u32,
    #[serde(default)]
    total_parts: u32,
}

fn resolve_binary(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let path = binaries_dir(app)?.join(name);
    if !path.exists() {
        return Err("필요한 도구가 아직 설치되지 않았습니다. 앱을 재시작하여 초기 설정을 완료해주세요.".into());
    }
    Ok(path)
}

fn make_command(path: PathBuf) -> tokio::process::Command {
    let mut std_cmd = std::process::Command::new(path);
    std_cmd.env("PYTHONUTF8", "1");
    std_cmd.env("PYTHONIOENCODING", "utf-8");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std_cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // 취소 시 자식 프로세스(yt-dlp가 띄우는 ffmpeg 등)까지 그룹 단위로 종료하기 위해
        // 새 process group의 리더로 실행 (kill -TERM -<pid>)
        std_cmd.process_group(0);
    }
    tokio::process::Command::from(std_cmd)
}

#[tauri::command]
async fn get_video_info(app: AppHandle, url: String) -> Result<VideoInfo, String> {
    let yt_dlp = resolve_binary(&app, &tool_name("yt-dlp"))?;
    let output = make_command(yt_dlp)
        .args(["-j", "--no-warnings", "--no-playlist", &url])
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    // Some sites (e.g. multi-part SOOP VODs) emit one JSON per line.
    // Take only the first non-empty line to get the lead video's info.
    let stdout_text = String::from_utf8_lossy(&output.stdout);
    let first_json_line = stdout_text
        .lines()
        .map(|l| l.trim())
        .find(|l| l.starts_with('{'))
        .ok_or("엔진이 영상 정보를 반환하지 않았습니다")?;

    let json: serde_json::Value =
        serde_json::from_str(first_json_line).map_err(|e| e.to_string())?;

    let formats = json["formats"].as_array().cloned().unwrap_or_default();

    let max_height = formats
        .iter()
        .filter(|f| f["vcodec"].as_str().map_or(false, |v| v != "none"))
        .filter_map(|f| f["height"].as_u64())
        .max()
        .unwrap_or(0);

    let (max_fps, max_vcodec) = formats
        .iter()
        .filter(|f| f["height"].as_u64() == Some(max_height))
        .filter(|f| f["vcodec"].as_str().map_or(false, |v| v != "none"))
        .map(|f| {
            (
                f["fps"].as_f64().unwrap_or(0.0),
                f["vcodec"].as_str().unwrap_or("").to_string(),
            )
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or((0.0, String::new()));

    let (max_abr, max_acodec) = formats
        .iter()
        .filter(|f| f["acodec"].as_str().map_or(false, |a| a != "none"))
        .map(|f| {
            (
                f["abr"].as_f64().unwrap_or(0.0),
                f["acodec"].as_str().unwrap_or("").to_string(),
            )
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or((0.0, String::new()));

    let is_live = json["is_live"].as_bool().unwrap_or(false)
        || json["live_status"].as_str() == Some("is_live")
        || json["live_status"].as_str() == Some("is_upcoming");

    let chapter_count = json["chapters"]
        .as_array()
        .map(|a| a.len() as u64)
        .unwrap_or(0);

    Ok(VideoInfo {
        title: json["title"].as_str().unwrap_or("(제목 없음)").to_string(),
        duration: json["duration"].as_f64().unwrap_or(0.0) as u64,
        thumbnail: json["thumbnail"].as_str().unwrap_or("").to_string(),
        uploader: json["uploader"].as_str().unwrap_or("").to_string(),
        view_count: json["view_count"].as_u64().unwrap_or(0),
        max_height,
        max_fps,
        max_abr,
        max_vcodec: simplify_codec(&max_vcodec),
        max_acodec: simplify_codec(&max_acodec),
        is_live,
        chapter_count,
    })
}

fn format_args(format_id: &str) -> (String, Vec<String>) {
    match format_id {
        "mp3" => (
            "bestaudio/best".into(),
            vec![
                "--extract-audio".into(),
                "--audio-format".into(),
                "mp3".into(),
                "--audio-quality".into(),
                "0".into(),
                "--embed-thumbnail".into(),
                "--add-metadata".into(),
            ],
        ),
        "mp4-2160" => ("bv*[height<=2160]+ba/b[height<=2160]/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        "mp4-1440" => ("bv*[height<=1440]+ba/b[height<=1440]/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        "mp4-1080" => ("bv*[height<=1080]+ba/b[height<=1080]/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        "mp4-720"  => ("bv*[height<=720]+ba/b[height<=720]/b".into(),  vec!["--merge-output-format".into(), "mp4".into()]),
        "mp4-480"  => ("bv*[height<=480]+ba/b[height<=480]/b".into(),  vec!["--merge-output-format".into(), "mp4".into()]),
        "mp4-best" => ("bv*+ba/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        // Premiere 편집용: 다운로드는 mp4-*와 동일하게 최고화질(AV1/VP9 포함)로 받고,
        // 다운로드 후 ProRes 422 HQ MOV로 후변환한다 (convert_to_prores)
        "premiere-2160" => ("bv*[height<=2160]+ba/b[height<=2160]/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        "premiere-1440" => ("bv*[height<=1440]+ba/b[height<=1440]/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        "premiere-1080" => ("bv*[height<=1080]+ba/b[height<=1080]/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        "premiere-720"  => ("bv*[height<=720]+ba/b[height<=720]/b".into(),  vec!["--merge-output-format".into(), "mp4".into()]),
        "premiere-480"  => ("bv*[height<=480]+ba/b[height<=480]/b".into(),  vec!["--merge-output-format".into(), "mp4".into()]),
        "premiere-best" => ("bv*+ba/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        "webm"     => ("bv*[ext=webm]+ba[ext=webm]/b[ext=webm]/b".into(), vec![]),
        "gif"      => ("bv*[height<=720]+ba/b[height<=720]/b".into(), vec!["--merge-output-format".into(), "mp4".into()]),
        _ => ("best".into(), vec![]),
    }
}

fn parse_progress(line: &str) -> Option<DownloadProgress> {
    let re = regex::Regex::new(
        r"\[download\]\s+([\d\.]+)%\s+of\s+~?\s*([\d\.]+\w+)\s+at\s+([\d\.]+\w+/s)\s+ETA\s+(\S+)"
    ).ok()?;

    if let Some(caps) = re.captures(line) {
        return Some(DownloadProgress {
            percent: caps.get(1)?.as_str().parse().ok()?,
            total_size: caps.get(2)?.as_str().to_string(),
            speed: caps.get(3)?.as_str().to_string(),
            eta: caps.get(4)?.as_str().to_string(),
            status: "downloading".into(),
            ..Default::default()
        });
    }

    if line.contains("[Merger]") {
        return Some(DownloadProgress {
            percent: 99.0,
            status: "merging".into(),
            message: "영상+음원 병합 중...".into(),
            ..Default::default()
        });
    }
    if line.contains("[Concat]") {
        return Some(DownloadProgress {
            percent: 99.5,
            status: "merging".into(),
            message: "파트 합치는 중...".into(),
            ..Default::default()
        });
    }
    if line.contains("[ExtractAudio]") {
        return Some(DownloadProgress {
            percent: 99.0,
            status: "converting".into(),
            message: "오디오 추출 중...".into(),
            ..Default::default()
        });
    }
    if line.contains("[EmbedThumbnail]") || line.contains("[Metadata]") {
        return Some(DownloadProgress {
            percent: 99.5,
            status: "tagging".into(),
            message: "메타데이터 입력 중...".into(),
            ..Default::default()
        });
    }
    if line.contains("[Fixup") || line.contains("[VideoConvertor]") {
        return Some(DownloadProgress {
            percent: 99.5,
            status: "converting".into(),
            message: "최종 처리 중...".into(),
            ..Default::default()
        });
    }
    None
}

// Premiere 편집용 후변환: 받은 최고화질 파일을 ProRes 422 HQ MOV + PCM 48kHz로 변환.
// 원본이 이미 손실압축이라 엄밀한 무손실은 아니고, 화질을 보존하는 편집용 중간코덱 변환.
// 임시(.mov.part)에 쓰고 성공 검증 후에만 최종 .mov로 교체. 성공 시 원본 삭제, 실패 시 원본 보존.
async fn convert_to_prores(
    app: &AppHandle,
    ffmpeg: &PathBuf,
    src_str: &str,
    fallback_duration: u64,
) -> Result<(), String> {
    let src = PathBuf::from(src_str);
    if !src.exists() {
        return Err("변환할 원본 파일을 찾을 수 없습니다".into());
    }
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("output").to_string();
    let parent = src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
    let mov_path = parent.join(format!("{}.mov", stem));
    let part_path = parent.join(format!("{}.mov.part", stem));
    let _ = std::fs::remove_file(&part_path);

    // 진행률 분모는 실제 파일 길이(ffprobe) — 구간 자르기/SponsorBlock으로 원본 메타보다 짧을 수 있음
    let mut total_secs = fallback_duration as f64;
    let ffprobe = ffmpeg.parent().map(|p| p.join(tool_name("ffprobe")));
    if let Some(fp) = ffprobe.filter(|p| p.exists()) {
        if let Ok(out) = make_command(fp)
            .args([
                "-v", "error",
                "-show_entries", "format=duration",
                "-of", "default=nw=1:nk=1",
                src.to_str().unwrap_or(""),
            ])
            .output()
            .await
        {
            if let Ok(s) = String::from_utf8(out.stdout) {
                if let Ok(d) = s.trim().parse::<f64>() {
                    if d > 0.0 {
                        total_secs = d;
                    }
                }
            }
        }
    }

    let _ = app.emit("download-progress", DownloadProgress {
        percent: 0.0,
        status: "converting".into(),
        message: "Premiere용 ProRes 변환 시작...".into(),
        ..Default::default()
    });

    let mut ff_cmd = make_command(ffmpeg.clone());
    ff_cmd.args([
        "-y",
        "-i",
        src.to_str().unwrap_or(""),
        "-c:v",
        "prores_ks",
        "-profile:v",
        "3",
        "-pix_fmt",
        "yuv422p10le",
        "-c:a",
        "pcm_s16le",
        "-ar",
        "48000",
        "-progress",
        "pipe:1",
        "-nostats",
        "-f",
        "mov",
        part_path.to_str().unwrap_or(""),
    ]);
    ff_cmd.stdout(std::process::Stdio::piped());
    ff_cmd.stderr(std::process::Stdio::piped());

    let mut child = ff_cmd.spawn().map_err(|e| e.to_string())?;
    // 취소 버튼(cancel_download)이 변환 중에도 듣도록 ffmpeg PID를 등록
    set_current_pid(child.id());
    let stdout = child.stdout.take().ok_or("stdout 없음")?;
    let stderr = child.stderr.take().ok_or("stderr 없음")?;

    let app_conv = app.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // -progress pipe:1 → "out_time=00:01:23.456789" 형식 키=값 줄
            if let Some(t) = line.strip_prefix("out_time=") {
                let parts: Vec<&str> = t.trim().split(':').collect();
                if parts.len() == 3 {
                    let h: f64 = parts[0].parse().unwrap_or(0.0);
                    let m: f64 = parts[1].parse().unwrap_or(0.0);
                    let s: f64 = parts[2].parse().unwrap_or(0.0);
                    let done = h * 3600.0 + m * 60.0 + s;
                    let (percent, msg) = if total_secs > 0.0 {
                        let p = (done / total_secs * 100.0).clamp(0.0, 99.9);
                        (p, format!("Premiere용 ProRes 변환 중 · {:.0}%", p))
                    } else {
                        (99.0, format!("Premiere용 ProRes 변환 중 · {}", fmt_time(done)))
                    };
                    let _ = app_conv.emit("download-progress", DownloadProgress {
                        percent,
                        status: "converting".into(),
                        message: msg,
                        ..Default::default()
                    });
                }
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut all: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            all.push(line);
            if all.len() > 50 {
                all.remove(0);
            }
        }
        all.join("\n")
    });

    let status = child.wait().await.map_err(|e| e.to_string())?;
    set_current_pid(None);
    let _ = stdout_task.await;
    let stderr_output = stderr_task.await.unwrap_or_default();

    let part_ok = status.success()
        && part_path.exists()
        && std::fs::metadata(&part_path).map(|m| m.len() > 1024).unwrap_or(false);

    if part_ok {
        // 기존 동명 .mov가 있으면 교체 (Windows rename은 대상이 있으면 실패)
        if mov_path.exists() {
            let _ = std::fs::remove_file(&mov_path);
        }
        std::fs::rename(&part_path, &mov_path)
            .map_err(|e| format!("변환 파일 이동 실패: {}", e))?;
        if let Err(e) = std::fs::remove_file(&src) {
            let _ = app.emit("download-progress", DownloadProgress {
                percent: 99.9,
                status: "converting".into(),
                message: format!("변환 완료 · 원본({})은 삭제 실패로 남아 있음: {}", src.display(), e),
                ..Default::default()
            });
        }
        Ok(())
    } else {
        let _ = std::fs::remove_file(&part_path);
        let last = stderr_output.lines().last().unwrap_or("").to_string();
        Err(format!("ProRes 변환 실패 (원본 파일은 그대로 남아 있음): {}", last))
    }
}

#[tauri::command]
async fn download_video(app: AppHandle, opts: DownloadOptions) -> Result<String, String> {
    // Premiere 변환은 결과 파일이 하나라는 가정 위에 있어 다중 파일을 내는 챕터 분할과 함께 쓸 수 없다
    if opts.format_id.starts_with("premiere-") && opts.split_chapters {
        return Err("Premiere 편집용 형식은 챕터별 분할과 함께 쓸 수 없습니다. 챕터 분할을 끄거나 MP4 형식을 사용해주세요.".into());
    }
    let yt_dlp = resolve_binary(&app, &tool_name("yt-dlp"))?;
    let ffmpeg = resolve_binary(&app, &tool_name("ffmpeg"))?;
    let ffmpeg_dir = ffmpeg.parent().ok_or("ffmpeg 경로 오류")?.to_path_buf();

    let (format_spec, extra_args) = format_args(&opts.format_id);

    let output_template = PathBuf::from(&opts.output_dir)
        .join("%(title)s.%(ext)s")
        .to_string_lossy()
        .to_string();

    // Capture downloaded filepath via --print-to-file (NOT --print, which silences stdout)
    let fp_temp = std::env::temp_dir().join(format!(
        "mkydl-fp-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&fp_temp);
    let fp_temp_str = fp_temp.to_string_lossy().to_string();

    let mut cmd = make_command(yt_dlp);
    cmd.args([
        "--newline",
        "--no-playlist",
        "--no-mtime",
        "--no-color",
        "--encoding",
        "utf-8",
        "--windows-filenames",
        "--hls-prefer-native",
        "--fragment-retries",
        "10",
        "--retries",
        "10",
        "--retry-sleep",
        "fragment:exp=1:8",
        "--print-to-file",
        "after_move:%(filepath)s",
        &fp_temp_str,
        "--ffmpeg-location",
        ffmpeg_dir.to_str().unwrap_or(""),
        "-f",
        &format_spec,
        "-o",
        &output_template,
    ]);
    for a in extra_args {
        cmd.arg(a);
    }

    if opts.subtitles_enabled && opts.format_id != "mp3" {
        cmd.arg("--write-subs");
        if opts.subtitle_auto {
            cmd.arg("--write-auto-subs");
        }
        let langs = if opts.subtitle_langs.is_empty() {
            "ko,en".to_string()
        } else {
            opts.subtitle_langs.clone()
        };
        cmd.args(["--sub-langs", &langs]);
        cmd.args(["--convert-subs", "srt"]);
        if opts.subtitle_embed {
            cmd.arg("--embed-subs");
        }
    }

    if opts.sponsorblock {
        cmd.args(["--sponsorblock-remove", "sponsor,selfpromo,interaction,intro,outro,music_offtopic"]);
    }

    if !opts.rate_limit.trim().is_empty() {
        let rl = opts.rate_limit.trim();
        let formatted = if rl.ends_with('M') || rl.ends_with('K') || rl.ends_with('G') {
            rl.to_string()
        } else {
            format!("{}M", rl)
        };
        cmd.args(["--limit-rate", &formatted]);
    }

    if !opts.cookies_browser.trim().is_empty() {
        cmd.args(["--cookies-from-browser", opts.cookies_browser.trim()]);
    }

    if opts.split_chapters {
        cmd.arg("--split-chapters");
    }

    if !opts.download_section.trim().is_empty() {
        let section = format!("*{}", opts.download_section.trim());
        cmd.args(["--download-sections", &section]);
        // Note: omit --force-keyframes-at-cuts for broader compat (HLS multi-part).
        // Trade-off: cut may start at nearest keyframe (0.5-2s before requested time).
    }

    if opts.live_from_start {
        cmd.arg("--live-from-start");
        cmd.args(["--wait-for-video", "0"]);
    }

    cmd.arg(&opts.url);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    set_current_pid(child.id());
    let stdout = child.stdout.take().ok_or("stdout 없음")?;
    let stderr = child.stderr.take().ok_or("stderr 없음")?;

    let app_stdout = app.clone();
    let stdout_task = tokio::spawn(async move {
        let part_re = regex::Regex::new(r"Downloading item (\d+) of (\d+)").unwrap();
        let mut current_part: u32 = 0;
        let mut total_parts: u32 = 0;

        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(caps) = part_re.captures(&line) {
                current_part = caps.get(1).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
                total_parts = caps.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(0);
                let _ = app_stdout.emit(
                    "download-progress",
                    DownloadProgress {
                        percent: 0.0,
                        status: "downloading".into(),
                        message: format!("파트 {}/{} 시작", current_part, total_parts),
                        current_part,
                        total_parts,
                        ..Default::default()
                    },
                );
                continue;
            }

            if let Some(mut progress) = parse_progress(&line) {
                progress.current_part = current_part;
                progress.total_parts = total_parts;
                let _ = app_stdout.emit("download-progress", progress);
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut all_lines: Vec<String> = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            all_lines.push(line);
            if all_lines.len() > 300 {
                all_lines.remove(0);
            }
        }
        all_lines.join("\n")
    });

    let status = child.wait().await.map_err(|e| e.to_string())?;
    set_current_pid(None);
    let _ = stdout_task.await;
    let stderr_output = stderr_task.await.unwrap_or_default();

    // Read captured filepath from --print-to-file (last non-empty line)
    let captured_filepath: Option<String> = std::fs::read_to_string(&fp_temp)
        .ok()
        .and_then(|s| {
            s.lines()
                .rev()
                .map(|l| l.trim().to_string())
                .find(|l| !l.is_empty())
        });
    let _ = std::fs::remove_file(&fp_temp);

    if status.success() {
        if opts.format_id.starts_with("premiere-") {
            let Some(src_str) = captured_filepath.clone() else {
                return Err("다운로드 결과 파일 경로를 확인할 수 없어 ProRes 변환을 진행할 수 없습니다. (같은 영상을 이미 받았다면 기존 파일을 지우고 다시 시도해주세요)".into());
            };
            convert_to_prores(&app, &ffmpeg, &src_str, opts.duration_seconds).await?;
        }
        if opts.format_id == "gif" {
            if let Some(mp4_str) = captured_filepath.clone() {
                let _ = app.emit("download-progress", DownloadProgress {
                    percent: 99.0,
                    status: "converting".into(),
                    message: "GIF 변환 중 (ffmpeg)...".into(),
                    ..Default::default()
                });

                let mp4 = PathBuf::from(&mp4_str);
                let stem = mp4.file_stem().and_then(|s| s.to_str()).unwrap_or("output").to_string();
                let parent = mp4.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
                let gif_path = parent.join(format!("{}.gif", stem));

                let fps = if opts.gif_fps == 0 { 15 } else { opts.gif_fps };
                let width = if opts.gif_width == 0 { 480 } else { opts.gif_width };
                let vf = format!("fps={},scale={}:-1:flags=lanczos", fps, width);

                let mut ff_cmd = make_command(ffmpeg.clone());
                ff_cmd.args([
                    "-y",
                    "-i",
                    mp4.to_str().unwrap_or(""),
                    "-vf",
                    &vf,
                    "-loop",
                    "0",
                    gif_path.to_str().unwrap_or(""),
                ]);
                ff_cmd.stdout(std::process::Stdio::null());
                ff_cmd.stderr(std::process::Stdio::piped());

                let ff_output = ff_cmd.output().await.map_err(|e| e.to_string())?;
                if ff_output.status.success() {
                    let _ = std::fs::remove_file(&mp4);
                } else {
                    let err = String::from_utf8_lossy(&ff_output.stderr);
                    let last = err.lines().last().unwrap_or("").to_string();
                    return Err(format!("GIF 변환 실패: {}", last));
                }
            }
        }

        let _ = app.emit("download-progress", DownloadProgress {
            percent: 100.0,
            status: "finished".into(),
            message: "완료!".into(),
            ..Default::default()
        });
        Ok("ok".into())
    } else {
        let detail = if stderr_output.trim().is_empty() {
            "다운로드 실패 (출력 없음)".to_string()
        } else {
            stderr_output
        };
        Err(detail)
    }
}

#[tauri::command]
async fn open_folder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("explorer");
        cmd.arg(&path);
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("URL이 http(s)로 시작해야 합니다".into());
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("rundll32");
        cmd.args(["url.dll,FileProtocolHandler", &url]);
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Serialize, Clone)]
struct Announcement {
    id: String,
    title: String,
    body: String,
    image_url: String,
    link_url: String,
    button_text: String,
    show_from: String,
    show_until: String,
    accent: String,
}

#[tauri::command]
async fn fetch_announcement() -> Result<Option<Announcement>, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get("https://mkvibe.com/megabite/announcement.json")
        .send()
        .await;

    let resp = match resp {
        Ok(r) if r.status().is_success() => r,
        _ => return Ok(None),
    };

    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return Ok(None),
    };

    if json["id"].as_str().unwrap_or("").is_empty() {
        return Ok(None);
    }

    Ok(Some(Announcement {
        id: json["id"].as_str().unwrap_or("").to_string(),
        title: json["title"].as_str().unwrap_or("").to_string(),
        body: json["body"].as_str().unwrap_or("").to_string(),
        image_url: json["image_url"].as_str().unwrap_or("").to_string(),
        link_url: json["link_url"].as_str().unwrap_or("").to_string(),
        button_text: json["button_text"].as_str().unwrap_or("자세히 보기").to_string(),
        show_from: json["show_from"].as_str().unwrap_or("").to_string(),
        show_until: json["show_until"].as_str().unwrap_or("").to_string(),
        accent: json["accent"].as_str().unwrap_or("").to_string(),
    }))
}

#[derive(Serialize, Clone)]
struct AppUpdateStatus {
    current_version: String,
    latest_version: String,
    update_available: bool,
    release_notes_url: String,
}

async fn fetch_app_version_json() -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    client
        .get(APP_VERSION_URL)
        .send()
        .await
        .map_err(|e| format!("버전 정보 요청 실패: {}", e))?
        .error_for_status()
        .map_err(|e| format!("버전 정보 응답 오류: {}", e))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("버전 정보 파싱 실패: {}", e))
}

// 플랫폼에 맞는 version.json 항목 선택.
// Windows는 하위호환(이미 배포된 빌드)을 위해 기존 app.* 를 그대로 읽고,
// macOS는 platforms.<arch키> → platforms."macos-universal" 순서로 찾는다.
// arch 항목이 있어도 필수 필드(version/download_url/sha256)가 빠졌으면 universal로 넘어간다.
#[cfg(target_os = "macos")]
fn version_entry_complete(e: &serde_json::Value) -> bool {
    e.is_object()
        && e["version"].as_str().map_or(false, |s| !s.trim().is_empty())
        && e["download_url"].as_str().map_or(false, |s| !s.trim().is_empty())
        && e["sha256"].as_str().map_or(false, |s| !s.trim().is_empty())
}

fn select_version_entry(json: &serde_json::Value) -> serde_json::Value {
    #[cfg(windows)]
    {
        json["app"].clone()
    }
    #[cfg(target_os = "macos")]
    {
        let p = &json["platforms"];
        let e = &p[PLATFORM_KEY];
        if version_entry_complete(e) {
            e.clone()
        } else {
            p["macos-universal"].clone()
        }
    }
}

#[tauri::command]
async fn check_app_update(app: AppHandle) -> Result<AppUpdateStatus, String> {
    let current = app.package_info().version.clone();
    let json = fetch_app_version_json().await?;
    let entry = select_version_entry(&json);
    #[cfg(target_os = "macos")]
    if !entry.is_object() {
        // 맥용 배포 항목이 아직 없으면 "업데이트 없음"으로 조용히 처리
        return Ok(AppUpdateStatus {
            current_version: current.to_string(),
            latest_version: current.to_string(),
            update_available: false,
            release_notes_url: String::new(),
        });
    }
    // "v1.0.2" 표기 방어: 앞의 v는 벗기고 bare semver로 비교
    let latest_str = entry["version"].as_str().unwrap_or("").trim().trim_start_matches('v').to_string();
    let latest = semver::Version::parse(&latest_str)
        .map_err(|e| format!("버전 형식 오류({}): {}", latest_str, e))?;
    let update_available = latest > current;
    Ok(AppUpdateStatus {
        current_version: current.to_string(),
        latest_version: latest_str,
        update_available,
        release_notes_url: entry["release_notes_url"].as_str().unwrap_or("").to_string(),
    })
}

#[tauri::command]
async fn install_app_update(app: AppHandle) -> Result<String, String> {
    // 설치 시점에 version.json을 다시 읽어 최신 URL/해시 기준으로 진행 + 버전 재확인
    let current = app.package_info().version.clone();
    let json = fetch_app_version_json().await?;
    let a = select_version_entry(&json);
    #[cfg(target_os = "macos")]
    if !a.is_object() {
        return Err("이 플랫폼용 배포 정보가 아직 없습니다".into());
    }

    let latest_str = a["version"].as_str().unwrap_or("").trim().trim_start_matches('v').to_string();
    let latest = semver::Version::parse(&latest_str)
        .map_err(|e| format!("버전 형식 오류({}): {}", latest_str, e))?;
    if latest <= current {
        return Err(format!("이미 최신 버전입니다 (현재 v{}, 배포 v{})", current, latest));
    }

    let download_url = a["download_url"].as_str().unwrap_or("").to_string();
    #[cfg(windows)]
    if !download_url.starts_with("https://mkvibe.com/") || !download_url.to_lowercase().ends_with(".exe") {
        return Err(format!("허용되지 않는 다운로드 주소입니다: {}", download_url));
    }
    #[cfg(target_os = "macos")]
    if !download_url.starts_with("https://mkvibe.com/") || !download_url.to_lowercase().ends_with(".dmg") {
        return Err(format!("허용되지 않는 다운로드 주소입니다: {}", download_url));
    }

    // sha256은 필수 — 없으면 설치하지 않는다 (배포 체크리스트에서 version.json에 기재)
    let expected_sha256 = a["sha256"].as_str().unwrap_or("").trim().to_lowercase();
    if expected_sha256.len() != 64 || !expected_sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("version.json에 유효한 sha256(64자리 hex)이 없어 설치를 진행하지 않습니다".into());
    }

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    let _ = app.emit("update-progress", "새 버전 설치 파일 다운로드 중...");
    let bytes = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("설치 파일 다운로드 실패: {}", e))?
        .error_for_status()
        .map_err(|e| format!("설치 파일 응답 오류: {}", e))?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let _ = app.emit("update-progress", "체크섬 검증 중...");
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = hex::encode(hasher.finalize());
    if actual != expected_sha256 {
        return Err(format!(
            "설치 파일 체크섬 불일치. 파일이 변조됐을 수 있어 설치를 거부합니다.\n예상: {}\n실제: {}",
            expected_sha256, actual
        ));
    }

    // 파일명은 URL 마지막 조각에서 안전한 문자만 취한다
    #[cfg(windows)]
    const INSTALLER_EXT: &str = ".exe";
    #[cfg(target_os = "macos")]
    const INSTALLER_EXT: &str = ".dmg";
    let raw_name = download_url.rsplit('/').next().unwrap_or("");
    let mut file_name: String = raw_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    if !file_name.to_lowercase().ends_with(INSTALLER_EXT) {
        file_name = format!("MKDownloader-setup{}", INSTALLER_EXT);
    }
    let installer_path = std::env::temp_dir().join(&file_name);
    tokio::fs::write(&installer_path, &bytes)
        .await
        .map_err(|e| format!("설치 파일 저장 실패: {}", e))?;

    #[cfg(windows)]
    {
        let _ = app.emit("update-progress", "설치 프로그램 실행 중...");
        // 실행 중인 exe를 설치기가 건드리는 레이스를 피하기 위해,
        // 헬퍼(powershell)가 현재 프로세스 종료를 기다린 뒤 설치기를 띄운다
        let current_pid = std::process::id();
        // PS 단일따옴표 문자열 이스케이프: ' → '' (경로에 따옴표가 든 드문 환경 방어)
        let installer_path_ps = installer_path.display().to_string().replace('\'', "''");
        let script = format!(
            "Wait-Process -Id {} -ErrorAction SilentlyContinue; Start-Process -FilePath '{}'",
            current_pid, installer_path_ps
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]);
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn().map_err(|e| format!("설치 헬퍼 실행 실패: {}", e))?;

        // 앱을 종료하면 헬퍼가 설치기를 실행한다
        app.exit(0);
        Ok("installing".into())
    }
    #[cfg(target_os = "macos")]
    {
        // macOS는 자동 교체하지 않는다 — 검증된 DMG를 열어주고 사용자가 교체 (codex 협의 결정)
        let _ = app.emit("update-progress", "DMG 여는 중...");
        std::process::Command::new("open")
            .arg(&installer_path)
            .spawn()
            .map_err(|e| format!("DMG 열기 실패: {}", e))?;
        Ok("dmg-opened".into())
    }
}

#[derive(Serialize, Clone)]
struct UpdateStatus {
    current_version: String,
    latest_version: String,
    update_available: bool,
}

#[tauri::command]
async fn check_yt_dlp_update(app: AppHandle) -> Result<UpdateStatus, String> {
    let yt_dlp = resolve_binary(&app, &tool_name("yt-dlp"))?;
    let output = make_command(yt_dlp)
        .arg("--version")
        .output()
        .await
        .map_err(|e| e.to_string())?;
    let current = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let resp: serde_json::Value = client
        .get(GITHUB_LATEST_API)
        .send()
        .await
        .map_err(|e| format!("깃허브 API 요청 실패: {}", e))?
        .json()
        .await
        .map_err(|e| format!("깃허브 응답 파싱 실패: {}", e))?;

    let latest = resp["tag_name"].as_str().unwrap_or("").to_string();
    let update_available = !latest.is_empty() && !current.is_empty() && latest != current;

    Ok(UpdateStatus {
        current_version: current,
        latest_version: latest,
        update_available,
    })
}

#[tauri::command]
async fn update_yt_dlp(app: AppHandle) -> Result<String, String> {
    let yt_dlp_path = resolve_binary(&app, &tool_name("yt-dlp"))?;

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let _ = app.emit("update-progress", "체크섬 다운로드 중...");
    let sums_text = client
        .get(YT_DLP_SUMS_URL)
        .send()
        .await
        .map_err(|e| format!("체크섬 다운로드 실패: {}", e))?
        .text()
        .await
        .map_err(|e| e.to_string())?;

    let expected_hash = sums_text
        .lines()
        .find(|l| {
            let parts: Vec<&str> = l.split_whitespace().collect();
            parts.len() == 2 && parts[1] == yt_dlp_asset_name()
        })
        .and_then(|l| l.split_whitespace().next())
        .ok_or("SHA256SUMS에서 다운로드 엔진 항목을 찾을 수 없습니다")?
        .to_lowercase();

    let _ = app.emit("update-progress", "엔진 다운로드 중...");
    let bytes = client
        .get(&yt_dlp_download_url())
        .send()
        .await
        .map_err(|e| format!("엔진 다운로드 실패: {}", e))?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;

    let _ = app.emit("update-progress", "체크섬 검증 중...");
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual_hash = hex::encode(hasher.finalize());

    if actual_hash != expected_hash {
        return Err(format!(
            "⚠ 체크섬 불일치! 파일이 변조됐을 수 있어 적용을 거부합니다.\n예상: {}\n실제: {}",
            expected_hash, actual_hash
        ));
    }

    let _ = app.emit("update-progress", "파일 교체 중...");
    let temp_path = yt_dlp_path.with_extension("exe.new");
    tokio::fs::write(&temp_path, &bytes)
        .await
        .map_err(|e| format!("임시 파일 쓰기 실패: {}", e))?;
    // macOS: 교체 전에 임시 파일 상태에서 마무리 (실패 시 기존 파일 보존)
    #[cfg(target_os = "macos")]
    if let Err(e) = finalize_mac_binary(&temp_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(e);
    }
    tokio::fs::rename(&temp_path, &yt_dlp_path)
        .await
        .map_err(|e| format!("파일 교체 실패 (다운로드 중이면 종료 후 다시 시도): {}", e))?;

    let new_version = String::from_utf8_lossy(
        &make_command(yt_dlp_path)
            .arg("--version")
            .output()
            .await
            .map_err(|e| e.to_string())?
            .stdout,
    )
    .trim()
    .to_string();

    Ok(new_version)
}

#[derive(Serialize, Clone)]
struct SetupProgress {
    step: String,
    percent: f64,
    downloaded_mb: f64,
    total_mb: f64,
    message: String,
}

#[tauri::command]
async fn check_binaries_ready(app: AppHandle) -> Result<bool, String> {
    let bin = binaries_dir(&app)?;
    Ok(bin.join(tool_name("yt-dlp")).exists()
        && bin.join(tool_name("ffmpeg")).exists()
        && bin.join(tool_name("ffprobe")).exists())
}

async fn download_with_progress(
    app: &AppHandle,
    url: &str,
    target: &std::path::Path,
    step: &str,
    label: &str,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(0);
    let total_mb = total as f64 / (1024.0 * 1024.0);

    let mut file = tokio::fs::File::create(target).await.map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_emit = std::time::Instant::now();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;

        if last_emit.elapsed().as_millis() > 50 {
            let dl_mb = downloaded as f64 / (1024.0 * 1024.0);
            let pct = if total > 0 {
                (downloaded as f64 / total as f64 * 100.0).min(100.0)
            } else {
                0.0
            };
            let _ = app.emit(
                "setup-progress",
                SetupProgress {
                    step: step.into(),
                    percent: pct,
                    downloaded_mb: dl_mb,
                    total_mb,
                    message: format!("{} · {:.1}MB / {:.1}MB", label, dl_mb, total_mb),
                },
            );
            last_emit = std::time::Instant::now();
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;

    // Guarantee a final 100% emit so the UI bar visibly fills before next step
    let final_mb = downloaded as f64 / (1024.0 * 1024.0);
    let _ = app.emit(
        "setup-progress",
        SetupProgress {
            step: step.into(),
            percent: 100.0,
            downloaded_mb: final_mb,
            total_mb: if total_mb > 0.0 { total_mb } else { final_mb },
            message: format!("{} 완료 · {:.1}MB", label, final_mb),
        },
    );
    // Give the UI a moment to render the full bar before the next step resets it
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;

    Ok(())
}

#[tauri::command]
async fn setup_binaries(app: AppHandle) -> Result<String, String> {
    let bin = binaries_dir(&app)?;
    std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;

    let yt_path = bin.join(tool_name("yt-dlp"));
    let ffmpeg_path = bin.join(tool_name("ffmpeg"));
    let ffprobe_path = bin.join(tool_name("ffprobe"));

    if !yt_path.exists() {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| e.to_string())?;

        let _ = app.emit(
            "setup-progress",
            SetupProgress {
                step: "yt-dlp".into(),
                percent: 0.0,
                downloaded_mb: 0.0,
                total_mb: 0.0,
                message: "엔진 체크섬 가져오는 중...".into(),
            },
        );

        let sums = client
            .get(YT_DLP_SUMS_URL)
            .send()
            .await
            .map_err(|e| format!("체크섬 다운로드 실패: {}", e))?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let expected_hash = sums
            .lines()
            .find(|l| {
                let p: Vec<&str> = l.split_whitespace().collect();
                p.len() == 2 && p[1] == yt_dlp_asset_name()
            })
            .and_then(|l| l.split_whitespace().next())
            .ok_or("SHA256SUMS에서 다운로드 엔진 항목을 찾을 수 없음")?
            .to_lowercase();

        // Use a temp file in the same directory as the destination to avoid
        // ERROR_NOT_SAME_DEVICE (os error 17) when %TEMP% and %LocalAppData% are on different drives
        let temp_yt = bin.join(format!("{}.tmp", tool_name("yt-dlp")));
        let _ = std::fs::remove_file(&temp_yt);
        download_with_progress(&app, &yt_dlp_download_url(), &temp_yt, "yt-dlp", "다운로드 엔진").await?;

        let bytes = tokio::fs::read(&temp_yt).await.map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let actual = hex::encode(hasher.finalize());
        if actual != expected_hash {
            let _ = std::fs::remove_file(&temp_yt);
            return Err(format!(
                "다운로드 엔진 체크섬 불일치 (변조 의심)\n예상: {}\n실제: {}",
                expected_hash, actual
            ));
        }
        // macOS: 최종 위치로 옮기기 전에 임시 파일 상태에서 마무리 (실패 시 최종 파일이 안 생기게)
        #[cfg(target_os = "macos")]
        if let Err(e) = finalize_mac_binary(&temp_yt) {
            let _ = std::fs::remove_file(&temp_yt);
            return Err(e);
        }
        tokio::fs::rename(&temp_yt, &yt_path)
            .await
            .map_err(|e| e.to_string())?;
    }

    #[cfg(windows)]
    if !ffmpeg_path.exists() || !ffprobe_path.exists() {
        // Same-drive temp to avoid cross-drive rename failures
        let temp_zip = bin.join("ffmpeg.zip.tmp");
        let _ = std::fs::remove_file(&temp_zip);
        download_with_progress(
            &app,
            FFMPEG_ZIP_URL,
            &temp_zip,
            "ffmpeg",
            "ffmpeg 압축 파일",
        )
        .await?;

        let _ = app.emit(
            "setup-progress",
            SetupProgress {
                step: "extract".into(),
                percent: 0.0,
                downloaded_mb: 0.0,
                total_mb: 0.0,
                message: "압축 해제 시작...".into(),
            },
        );

        let bin_clone = bin.clone();
        let zip_clone = temp_zip.clone();
        let app_extract = app.clone();
        let extract_result = tokio::task::spawn_blocking(move || -> Result<(), String> {
            use std::io::{Read, Write};

            let file = std::fs::File::open(&zip_clone).map_err(|e| e.to_string())?;
            let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

            let is_target = |name: &str| -> bool {
                name.ends_with("/ffmpeg.exe")
                    || name == "ffmpeg.exe"
                    || name.ends_with("/ffprobe.exe")
                    || name == "ffprobe.exe"
            };

            // First pass: total bytes to extract (for accurate %)
            let mut total_bytes: u64 = 0;
            for i in 0..archive.len() {
                let entry = archive.by_index(i).map_err(|e| e.to_string())?;
                if is_target(entry.name()) {
                    total_bytes += entry.size();
                }
            }
            let total_mb = total_bytes as f64 / (1024.0 * 1024.0);

            // Second pass: extract with chunked progress
            let mut extracted: u64 = 0;
            let mut last_emit = std::time::Instant::now();
            let mut buf = vec![0u8; 64 * 1024];

            for i in 0..archive.len() {
                let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
                let name = entry.name().to_string();
                let target_path = if name.ends_with("/ffmpeg.exe") || name == "ffmpeg.exe" {
                    Some(bin_clone.join("ffmpeg.exe"))
                } else if name.ends_with("/ffprobe.exe") || name == "ffprobe.exe" {
                    Some(bin_clone.join("ffprobe.exe"))
                } else {
                    None
                };
                if let Some(out_path) = target_path {
                    let mut out_file =
                        std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
                    loop {
                        let n = entry.read(&mut buf).map_err(|e| e.to_string())?;
                        if n == 0 {
                            break;
                        }
                        out_file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                        extracted += n as u64;

                        if last_emit.elapsed().as_millis() > 50 {
                            let pct = if total_bytes > 0 {
                                (extracted as f64 / total_bytes as f64 * 100.0).min(99.0)
                            } else {
                                0.0
                            };
                            let mb = extracted as f64 / (1024.0 * 1024.0);
                            let _ = app_extract.emit(
                                "setup-progress",
                                SetupProgress {
                                    step: "extract".into(),
                                    percent: pct,
                                    downloaded_mb: mb,
                                    total_mb,
                                    message: format!(
                                        "압축 해제 · {:.1}MB / {:.1}MB",
                                        mb, total_mb
                                    ),
                                },
                            );
                            last_emit = std::time::Instant::now();
                        }
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| e.to_string())?;
        extract_result?;

        // Guaranteed 100% emit for extract + pause for UI render
        let _ = app.emit(
            "setup-progress",
            SetupProgress {
                step: "extract".into(),
                percent: 100.0,
                downloaded_mb: 0.0,
                total_mb: 0.0,
                message: "압축 해제 완료".into(),
            },
        );
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;

        let _ = std::fs::remove_file(&temp_zip);

        if !ffmpeg_path.exists() || !ffprobe_path.exists() {
            return Err("ffmpeg/ffprobe 추출 실패 (zip 안에서 못 찾음)".into());
        }
    }

    // macOS: 우리 GitHub Releases 미러(OSXExperts 검증본)에서 도구별 zip을 받아
    // zip 해시 → 추출 → 실행파일 해시 이중 검증 후 설치
    #[cfg(target_os = "macos")]
    if !ffmpeg_path.exists() || !ffprobe_path.exists() {
        for src in mac_ffmpeg_sources() {
            let dest = bin.join(src.member);
            if dest.exists() {
                continue;
            }
            let temp_zip = bin.join(format!("{}.zip.tmp", src.member));
            let _ = std::fs::remove_file(&temp_zip);
            download_with_progress(
                &app,
                src.url,
                &temp_zip,
                "ffmpeg",
                &format!("{} 압축 파일", src.member),
            )
            .await?;

            let zbytes = tokio::fs::read(&temp_zip).await.map_err(|e| e.to_string())?;
            let mut h = Sha256::new();
            h.update(&zbytes);
            let zhash = hex::encode(h.finalize());
            if zhash != src.archive_sha256 {
                let _ = std::fs::remove_file(&temp_zip);
                return Err(format!(
                    "{} 압축 파일 체크섬 불일치 (변조 의심)\n예상: {}\n실제: {}",
                    src.member, src.archive_sha256, zhash
                ));
            }

            let _ = app.emit(
                "setup-progress",
                SetupProgress {
                    step: "extract".into(),
                    percent: 0.0,
                    downloaded_mb: 0.0,
                    total_mb: 0.0,
                    message: format!("{} 압축 해제 중...", src.member),
                },
            );

            let member = src.member.to_string();
            let exe_bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
                use std::io::Read;
                let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zbytes))
                    .map_err(|e| e.to_string())?;
                let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();
                let found = names
                    .iter()
                    .find(|n| **n == member || n.ends_with(&format!("/{}", member)))
                    .cloned()
                    .ok_or(format!("zip에서 {} 를 찾을 수 없음", member))?;
                let mut entry = archive.by_name(&found).map_err(|e| e.to_string())?;
                let mut out = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut out).map_err(|e| e.to_string())?;
                Ok(out)
            })
            .await
            .map_err(|e| e.to_string())??;

            let mut h2 = Sha256::new();
            h2.update(&exe_bytes);
            let ehash = hex::encode(h2.finalize());
            if ehash != src.executable_sha256 {
                let _ = std::fs::remove_file(&temp_zip);
                return Err(format!(
                    "{} 실행파일 체크섬 불일치 (변조 의심)\n예상: {}\n실제: {}",
                    src.member, src.executable_sha256, ehash
                ));
            }

            // 임시 파일에 쓰고 → 마무리(chmod/xattr/codesign) → 원자적 rename
            let dest_tmp = bin.join(format!("{}.bin.tmp", src.member));
            let _ = std::fs::remove_file(&dest_tmp);
            tokio::fs::write(&dest_tmp, &exe_bytes)
                .await
                .map_err(|e| e.to_string())?;
            if let Err(e) = finalize_mac_binary(&dest_tmp) {
                let _ = std::fs::remove_file(&dest_tmp);
                let _ = std::fs::remove_file(&temp_zip);
                return Err(e);
            }
            tokio::fs::rename(&dest_tmp, &dest)
                .await
                .map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&temp_zip);
        }
        if !ffmpeg_path.exists() || !ffprobe_path.exists() {
            return Err("ffmpeg/ffprobe 설치 실패".into());
        }
    }

    let _ = app.emit(
        "setup-progress",
        SetupProgress {
            step: "done".into(),
            percent: 100.0,
            downloaded_mb: 0.0,
            total_mb: 0.0,
            message: "✅ 설치 완료".into(),
        },
    );

    Ok("ok".into())
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ShortsOptions {
    url: String,
    output_dir: String,
    shorts_count: u32,
    clip_duration: u32,
    crop_mode: String, // "center" or "blur"
    cookies_browser: String,
}

#[derive(Clone, Debug)]
struct ClipPeak {
    start_time: f64,
    end_time: f64,
}

fn find_peaks(json: &serde_json::Value, count: u32, duration: u32) -> Vec<ClipPeak> {
    let video_duration = json["duration"].as_f64().unwrap_or(0.0);
    let clip_dur = duration as f64;

    if let Some(hm) = json["heatmap"].as_array() {
        let mut sorted: Vec<(f64, f64, f64)> = hm
            .iter()
            .filter_map(|item| {
                Some((
                    item["start_time"].as_f64()?,
                    item["end_time"].as_f64()?,
                    item["value"].as_f64()?,
                ))
            })
            .collect();
        sorted.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        let min_spacing = clip_dur;
        let mut selected: Vec<ClipPeak> = Vec::new();

        for (s, e, _v) in sorted {
            let center = (s + e) / 2.0;
            let too_close = selected.iter().any(|p| {
                let p_center = (p.start_time + p.end_time) / 2.0;
                (center - p_center).abs() < min_spacing
            });
            if too_close {
                continue;
            }
            let half = clip_dur / 2.0;
            let mut clip_s = (center - half).max(0.0);
            let mut clip_e = clip_s + clip_dur;
            if clip_e > video_duration {
                clip_e = video_duration;
                clip_s = (clip_e - clip_dur).max(0.0);
            }
            selected.push(ClipPeak {
                start_time: clip_s,
                end_time: clip_e,
            });
            if selected.len() >= count as usize {
                break;
            }
        }

        if !selected.is_empty() {
            selected.sort_by(|a, b| {
                a.start_time
                    .partial_cmp(&b.start_time)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            return selected;
        }
    }

    let n = count as f64;
    let spacing = video_duration / (n + 1.0);
    (0..count)
        .map(|i| {
            let center = spacing * (i as f64 + 1.0);
            let half = clip_dur / 2.0;
            let s = (center - half).max(0.0);
            let e = (s + clip_dur).min(video_duration);
            ClipPeak {
                start_time: s,
                end_time: e,
            }
        })
        .collect()
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn fmt_time(sec: f64) -> String {
    let total = sec as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

#[tauri::command]
async fn generate_shorts(app: AppHandle, opts: ShortsOptions) -> Result<String, String> {
    let yt_dlp = resolve_binary(&app, &tool_name("yt-dlp"))?;
    let ffmpeg = resolve_binary(&app, &tool_name("ffmpeg"))?;
    let ffmpeg_dir = ffmpeg.parent().ok_or("ffmpeg 경로 오류")?.to_path_buf();

    let emit_progress = |percent: f64, status: &str, message: String, speed: String, eta: String, total_size: String| {
        let _ = app.emit(
            "download-progress",
            DownloadProgress {
                percent,
                speed,
                eta,
                total_size,
                status: status.into(),
                message,
                ..Default::default()
            },
        );
    };

    emit_progress(0.0, "downloading", "영상 정보 분석 중...".into(), String::new(), String::new(), String::new());

    let mut info_cmd = make_command(yt_dlp.clone());
    info_cmd.args(["-j", "--no-warnings", "--no-playlist"]);
    if !opts.cookies_browser.trim().is_empty() {
        info_cmd.args(["--cookies-from-browser", opts.cookies_browser.trim()]);
    }
    info_cmd.arg(&opts.url);

    let info_output = tokio::time::timeout(
        tokio::time::Duration::from_secs(60),
        info_cmd.output(),
    )
    .await
    .map_err(|_| "영상 정보 가져오기 60초 초과")?
    .map_err(|e| e.to_string())?;
    if !info_output.status.success() {
        return Err(String::from_utf8_lossy(&info_output.stderr).trim().to_string());
    }
    let info_text = String::from_utf8_lossy(&info_output.stdout);
    let first_json_line = info_text
        .lines()
        .map(|l| l.trim())
        .find(|l| l.starts_with('{'))
        .ok_or("엔진이 영상 정보를 반환하지 않았습니다")?;
    let json: serde_json::Value =
        serde_json::from_str(first_json_line).map_err(|e| e.to_string())?;

    let title = json["title"].as_str().unwrap_or("video").to_string();
    let safe_title = sanitize_filename(&title);
    let has_heatmap = json["heatmap"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
    let video_duration = json["duration"].as_f64().unwrap_or(0.0);

    let count = opts.shorts_count.clamp(1, 10);
    let duration = opts.clip_duration.clamp(5, 120);
    let peaks = find_peaks(&json, count, duration);

    if peaks.is_empty() {
        return Err("영상이 너무 짧거나 분석에 실패했습니다".into());
    }

    let total = peaks.len();
    let detection_msg = if has_heatmap {
        format!("유튜브 다시본 구간 데이터로 {}개 피크 선정", total)
    } else {
        format!("균등 분할로 {}개 구간 선정 (heatmap 없음)", total)
    };
    emit_progress(2.0, "downloading", detection_msg, String::new(), String::new(), String::new());

    let temp_full = std::env::temp_dir().join(format!(
        "mkshorts_src_{}.mp4",
        std::process::id()
    ));
    let temp_full_str = temp_full.to_string_lossy().to_string();
    let _ = std::fs::remove_file(&temp_full);
    for entry in std::fs::read_dir(std::env::temp_dir()).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("mkshorts_src_") && name != format!("mkshorts_src_{}.mp4", std::process::id()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }

    let approx_mb = (video_duration / 60.0 * 25.0) as u64;
    emit_progress(
        3.0,
        "downloading",
        format!(
            "원본 720p 다운로드 ({}분 영상, 예상 ~{}MB)",
            (video_duration / 60.0) as u64,
            approx_mb.max(1)
        ),
        String::new(), String::new(), String::new(),
    );

    let mut dl_cmd = make_command(yt_dlp.clone());
    dl_cmd.args([
        "--no-playlist",
        "--newline",
        "--no-color",
        "--no-mtime",
        "--encoding",
        "utf-8",
        "--windows-filenames",
        "--ffmpeg-location",
        ffmpeg_dir.to_str().unwrap_or(""),
        "--extractor-args",
        "youtube:player_client=web_safari,web,mweb,android_vr",
        "-f",
        "bv*[height<=720][protocol^=https]+ba[protocol^=https]/b[height<=720][protocol^=https]/bv*[height<=720]+ba/b[height<=720]/b",
        "--merge-output-format",
        "mp4",
        "-o",
        &temp_full_str,
    ]);
    if !opts.cookies_browser.trim().is_empty() {
        dl_cmd.args(["--cookies-from-browser", opts.cookies_browser.trim()]);
    }
    dl_cmd.arg(&opts.url);
    dl_cmd.stdout(std::process::Stdio::piped());
    dl_cmd.stderr(std::process::Stdio::piped());

    let mut dl_child = dl_cmd.spawn().map_err(|e| e.to_string())?;
    set_current_pid(dl_child.id());
    let dl_stdout = dl_child.stdout.take().ok_or("stdout 없음")?;
    let dl_stderr = dl_child.stderr.take().ok_or("stderr 없음")?;

    let app_dl = app.clone();
    let stdout_task = tokio::spawn(async move {
        let mut lines = BufReader::new(dl_stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(p) = parse_progress(&line) {
                let blended = 5.0 + (p.percent * 0.45);
                let _ = app_dl.emit(
                    "download-progress",
                    DownloadProgress {
                        percent: blended.min(50.0),
                        speed: p.speed.clone(),
                        eta: p.eta.clone(),
                        total_size: p.total_size.clone(),
                        status: "downloading".into(),
                        message: format!(
                            "원본 다운로드 · {:.1}%{}{}",
                            p.percent,
                            if p.speed.is_empty() { String::new() } else { format!(" · {}", p.speed) },
                            if p.eta.is_empty() { String::new() } else { format!(" · {} 남음", p.eta) }
                        ),
                        ..Default::default()
                    },
                );
            } else if line.contains("[Merger]") {
                let _ = app_dl.emit(
                    "download-progress",
                    DownloadProgress {
                        percent: 49.0,
                        status: "merging".into(),
                        message: "원본 영상+음원 병합 중...".into(),
                        ..Default::default()
                    },
                );
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(dl_stderr).lines();
        let mut errs = Vec::<String>::new();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.contains("ERROR") {
                errs.push(line);
            }
        }
        errs.join("\n")
    });

    let dl_timeout_secs = ((video_duration / 60.0).max(10.0) * 30.0) as u64 + 600;
    let wait_result = tokio::time::timeout(
        tokio::time::Duration::from_secs(dl_timeout_secs),
        dl_child.wait(),
    ).await;
    set_current_pid(None);
    let _ = stdout_task.await;
    let stderr_msg = stderr_task.await.unwrap_or_default();

    let dl_status = match wait_result {
        Ok(r) => r.map_err(|e| e.to_string())?,
        Err(_) => {
            // 자식(ffmpeg 등)까지 트리로 종료 — 실패 시 최후수단으로 직접 kill
            if let Some(pid) = dl_child.id() {
                if terminate_process_tree(pid).await.is_err() {
                    let _ = dl_child.kill().await;
                }
            } else {
                let _ = dl_child.kill().await;
            }
            let _ = std::fs::remove_file(&temp_full);
            return Err(format!(
                "원본 다운로드 {}분 초과 (네트워크/yt-dlp 응답 없음)",
                dl_timeout_secs / 60
            ));
        }
    };

    if !dl_status.success() {
        let _ = std::fs::remove_file(&temp_full);
        return Err(format!(
            "원본 다운로드 실패: {}",
            stderr_msg.lines().last().unwrap_or("엔진 종료 코드 비정상")
        ));
    }

    if !temp_full.exists() {
        let alt: Option<PathBuf> = std::fs::read_dir(std::env::temp_dir())
            .ok()
            .and_then(|dir| {
                dir.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .find(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with(&format!("mkshorts_src_{}", std::process::id())))
                            .unwrap_or(false)
                    })
            });
        if alt.is_none() {
            return Err("원본 임시 파일을 찾을 수 없습니다".into());
        }
    }

    emit_progress(
        50.0,
        "converting",
        format!("✅ 원본 받기 완료 ({} 시작)", "쇼츠 변환"),
        String::new(), String::new(), String::new(),
    );

    let conv_share = 50.0 / total as f64;

    for (idx, peak) in peaks.iter().enumerate() {
        let num = idx + 1;
        let base = 50.0 + (idx as f64) * conv_share;

        emit_progress(
            base + 1.0,
            "converting",
            format!(
                "쇼츠 {}/{} 변환 중 ({}~{})",
                num,
                total,
                fmt_time(peak.start_time),
                fmt_time(peak.end_time)
            ),
            String::new(), String::new(), String::new(),
        );

        let clip_dur = peak.end_time - peak.start_time;
        let out_file = PathBuf::from(&opts.output_dir)
            .join(format!("{} - Short {}.mp4", safe_title, num));
        let out_str = out_file.to_string_lossy().to_string();

        let mut ff_cmd = make_command(ffmpeg.clone());
        ff_cmd.args([
            "-y",
            "-ss",
            &peak.start_time.to_string(),
            "-t",
            &clip_dur.to_string(),
            "-i",
            &temp_full_str,
        ]);

        if opts.crop_mode == "blur" {
            ff_cmd.args([
                "-filter_complex",
                "[0:v]split[bg][fg];[bg]scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920,gblur=sigma=20[bg];[fg]scale=1080:1920:force_original_aspect_ratio=decrease[fg];[bg][fg]overlay=(W-w)/2:(H-h)/2",
            ]);
        } else {
            ff_cmd.args(["-vf", "crop=ih*9/16:ih,scale=1080:1920:flags=lanczos"]);
        }

        ff_cmd.args([
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "21",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
            &out_str,
        ]);
        ff_cmd.stdout(std::process::Stdio::null());
        ff_cmd.stderr(std::process::Stdio::piped());

        let ff_output = tokio::time::timeout(
            tokio::time::Duration::from_secs(300),
            ff_cmd.output(),
        )
        .await
        .map_err(|_| format!("쇼츠 {}/{} ffmpeg 변환 5분 초과", num, total))?
        .map_err(|e| e.to_string())?;

        if !ff_output.status.success() {
            let err = String::from_utf8_lossy(&ff_output.stderr);
            let _ = std::fs::remove_file(&temp_full);
            return Err(format!(
                "쇼츠 {}/{} 변환 실패: {}",
                num,
                total,
                err.lines().last().unwrap_or("")
            ));
        }

        emit_progress(
            50.0 + ((idx + 1) as f64) * conv_share,
            "converting",
            format!("쇼츠 {}/{} 완료", num, total),
            String::new(), String::new(), String::new(),
        );
    }

    let _ = std::fs::remove_file(&temp_full);

    let _ = app.emit(
        "download-progress",
        DownloadProgress {
            percent: 100.0,
            status: "finished".into(),
            message: format!("✨ 쇼츠 {}개 생성 완료!", total),
            ..Default::default()
        },
    );

    Ok(format!("{}개 생성", total))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            get_video_info,
            download_video,
            check_yt_dlp_update,
            update_yt_dlp,
            open_folder,
            open_url,
            generate_shorts,
            check_binaries_ready,
            setup_binaries,
            cancel_download,
            fetch_announcement,
            check_app_update,
            install_app_update
        ])
        .setup(|app| {
            let show_item = MenuItemBuilder::with_id("show", "창 보이기").build(app)?;
            let hide_item = MenuItemBuilder::with_id("hide", "창 숨기기").build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", "종료").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&show_item, &hide_item])
                .separator()
                .items(&[&quit_item])
                .build()?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("MK 유튜브 다운로더")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "hide" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            if let Some(window) = app.get_webview_window("main") {
                let window_clone = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = window_clone.hide();
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
