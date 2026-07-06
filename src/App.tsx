import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { downloadDir } from "@tauri-apps/api/path";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { readText } from "@tauri-apps/plugin-clipboard-manager";
import {
  sendNotification,
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import "./App.css";

type VideoInfo = {
  title: string;
  duration: number;
  thumbnail: string;
  uploader: string;
  view_count: number;
  max_height: number;
  max_fps: number;
  max_abr: number;
  max_vcodec: string;
  max_acodec: string;
  is_live: boolean;
  chapter_count: number;
};

type Progress = {
  percent: number;
  speed: string;
  eta: string;
  total_size: string;
  status: "downloading" | "merging" | "converting" | "tagging" | "finished";
  message: string;
  current_part?: number;
  total_parts?: number;
};

type FormatType = "mp4" | "premiere" | "mp3" | "webm" | "gif" | "shorts";
type Quality = "best" | "2160" | "1440" | "1080" | "720" | "480";
type CropMode = "center" | "blur";
type ShortsConfig = {
  count: number;
  duration: number;
  cropMode: CropMode;
};
const DEFAULT_SHORTS: ShortsConfig = {
  count: 3,
  duration: 30,
  cropMode: "center",
};

type UpdateStatus = {
  current_version: string;
  latest_version: string;
  update_available: boolean;
};

type AppUpdateStatus = {
  current_version: string;
  latest_version: string;
  update_available: boolean;
  release_notes_url: string;
};

type AdvancedOpts = {
  subtitlesEnabled: boolean;
  subtitleLangs: string;
  subtitleAuto: boolean;
  subtitleEmbed: boolean;
  sponsorblock: boolean;
  rateLimit: string;
  cookiesBrowser: string;
  splitChapters: boolean;
  sectionEnabled: boolean;
  downloadSection: string;
  liveFromStart: boolean;
  gifFps: number;
  gifWidth: number;
};

const DEFAULT_ADV: AdvancedOpts = {
  subtitlesEnabled: false,
  subtitleLangs: "ko,en",
  subtitleAuto: false,
  subtitleEmbed: false,
  sponsorblock: false,
  rateLimit: "",
  cookiesBrowser: "",
  splitChapters: false,
  sectionEnabled: false,
  downloadSection: "",
  liveFromStart: false,
  gifFps: 15,
  gifWidth: 480,
};

type LibraryItem = {
  id: string;
  title: string;
  thumbnail: string;
  formatLabel: string;
  outputDir: string;
  completedAt: number;
};

type QueueStatus = "pending" | "downloading" | "completed" | "failed";

type QueueItem = {
  id: string;
  url: string;
  info: VideoInfo;
  formatLabel: string;
  formatId: string;
  outputDir: string;
  adv: AdvancedOpts;
  shorts?: ShortsConfig;
  status: QueueStatus;
  progress?: Progress;
  error?: string;
};

// Known video platform URL extractor (for clipboard auto-detect + drag&drop).
// yt-dlp supports 1800+ sites; this list is the common ones for Korean users.
const VIDEO_URL_EXTRACT = /https?:\/\/[^\s'"<>]*(?:youtube\.com|youtu\.be|tiktok\.com|instagram\.com|twitter\.com|x\.com|fb\.watch|facebook\.com|vimeo\.com|twitch\.tv|sooplive\.com|sooplive\.co\.kr|afreecatv\.com|tv\.naver\.com|tv\.kakao\.com|dailymotion\.com|reddit\.com|bilibili\.com|soundcloud\.com|streamable\.com)[^\s'"<>]*/i;

// Any HTTP(S) URL — used for "is the URL field a URL at all?" check
const ANY_URL_RE = /^https?:\/\/\S+/i;

// Normalize URLs to formats yt-dlp recognizes.
// SOOP rebrand: .com works in browser but yt-dlp's extractor only matches .co.kr
function normalizeUrl(input: string): string {
  let u = input.trim();
  u = u.replace(/sooplive\.com/gi, "sooplive.co.kr");
  return u;
}

function formatDuration(s: number): string {
  if (!s) return "-";
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return h > 0
    ? `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}`
    : `${m}:${String(sec).padStart(2, "0")}`;
}

function formatViews(n: number): string {
  if (!n) return "-";
  if (n >= 100_000_000) return `${(n / 100_000_000).toFixed(1)}억회`;
  if (n >= 10_000) return `${(n / 10_000).toFixed(1)}만회`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}천회`;
  return `${n}회`;
}

function formatHeight(h: number): string {
  if (!h) return "-";
  if (h >= 4320) return "8K";
  if (h >= 2160) return "4K";
  if (h >= 1440) return "QHD";
  if (h >= 1080) return "FHD";
  if (h >= 720) return "HD";
  return `${h}p`;
}

function formatMaxVideo(info: VideoInfo): string {
  if (!info.max_height) return "-";
  const tier = formatHeight(info.max_height);
  const px = `${info.max_height}p`;
  const fps = info.max_fps >= 50 ? Math.round(info.max_fps).toString() : "";
  const codec = info.max_vcodec ? ` ${info.max_vcodec}` : "";
  return `${tier} (${px}${fps})${codec}`;
}

function formatMaxAudio(info: VideoInfo): string {
  if (!info.max_abr) return "-";
  const codec = info.max_acodec ? ` ${info.max_acodec}` : "";
  return `${Math.round(info.max_abr)} kbps${codec}`;
}

function qualityLabel(q: Quality): string {
  return q === "best" ? "최고" : q === "2160" ? "4K" : `${q}p`;
}

function formatLabelFor(type: FormatType, q: Quality, shorts?: ShortsConfig): string {
  if (type === "mp3") return "🎵 MP3";
  if (type === "webm") return "🌐 WebM";
  if (type === "gif") return "🎞 GIF";
  if (type === "shorts" && shorts) {
    return `✨ 쇼츠 ${shorts.count}개 × ${shorts.duration}초`;
  }
  if (type === "premiere") {
    return `✂️ Premiere ProRes ${qualityLabel(q)}`;
  }
  return `🎬 MP4 ${qualityLabel(q)}`;
}

function relativeTime(ts: number): string {
  const sec = Math.floor((Date.now() - ts) / 1000);
  if (sec < 60) return "방금";
  if (sec < 3600) return `${Math.floor(sec / 60)}분 전`;
  if (sec < 86400) return `${Math.floor(sec / 3600)}시간 전`;
  if (sec < 86400 * 7) return `${Math.floor(sec / 86400)}일 전`;
  const d = new Date(ts);
  return `${d.getMonth() + 1}/${d.getDate()}`;
}

function genId(): string {
  if (typeof crypto !== "undefined" && (crypto as any).randomUUID) {
    return (crypto as any).randomUUID();
  }
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

type SectionTime = { sH: number; sM: number; sS: number; eH: number; eM: number; eS: number };

function parseSectionTime(s: string): SectionTime {
  const m = s.match(/^(\d+):(\d+):(\d+)-(\d+):(\d+):(\d+)$/);
  if (m) {
    return { sH: +m[1], sM: +m[2], sS: +m[3], eH: +m[4], eM: +m[5], eS: +m[6] };
  }
  return { sH: 0, sM: 0, sS: 0, eH: 0, eM: 0, eS: 0 };
}

function formatSectionTime(t: SectionTime): string {
  const total = t.sH + t.sM + t.sS + t.eH + t.eM + t.eS;
  if (total === 0) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(t.sH)}:${p(t.sM)}:${p(t.sS)}-${p(t.eH)}:${p(t.eM)}:${p(t.eS)}`;
}

function TimeUnit({ value, max, onChange }: { value: number; max: number; onChange: (n: number) => void }) {
  return (
    <input
      type="text"
      inputMode="numeric"
      value={String(value).padStart(2, "0")}
      onFocus={(e) => e.target.select()}
      onChange={(e) => {
        // Strip non-digits, keep last 2 (so 3rd typed digit replaces oldest)
        const digits = e.target.value.replace(/\D/g, "").slice(-2);
        const n = digits ? parseInt(digits, 10) : 0;
        onChange(Math.min(n, max));
      }}
      className="input time-unit"
    />
  );
}

type SetupProgress = {
  step: string;
  percent: number;
  downloaded_mb: number;
  total_mb: number;
  message: string;
};

type Announcement = {
  id: string;
  title: string;
  body: string;
  image_url: string;
  link_url: string;
  button_text: string;
  show_from: string;
  show_until: string;
  accent: string;
};

function App() {
  const [binariesReady, setBinariesReady] = useState<boolean | null>(null);
  const [setupRunning, setSetupRunning] = useState(false);
  const [setupProgress, setSetupProgress] = useState<SetupProgress | null>(null);
  const [setupError, setSetupError] = useState<string | null>(null);
  const [announcement, setAnnouncement] = useState<Announcement | null>(null);

  const [url, setUrl] = useState("");
  const [info, setInfo] = useState<VideoInfo | null>(null);
  const [loadingInfo, setLoadingInfo] = useState(false);
  const [formatType, setFormatType] = useState<FormatType>("mp4");
  const [quality, setQuality] = useState<Quality>("1080");
  const [outputDir, setOutputDir] = useState<string>(
    () => localStorage.getItem("outputDir") ?? ""
  );
  const [error, setError] = useState<string | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus | null>(null);
  const [updating, setUpdating] = useState(false);
  const [updateMsg, setUpdateMsg] = useState<string>("");
  const [updateDone, setUpdateDone] = useState<string | null>(null);
  const [appUpdate, setAppUpdate] = useState<AppUpdateStatus | null>(null);
  const [appUpdating, setAppUpdating] = useState(false);
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const [advOpen, setAdvOpen] = useState<boolean>(
    () => localStorage.getItem("advOpen") === "1"
  );
  const [adv, setAdv] = useState<AdvancedOpts>(() => {
    try {
      const saved = localStorage.getItem("adv");
      if (saved) return { ...DEFAULT_ADV, ...JSON.parse(saved) };
    } catch {}
    return DEFAULT_ADV;
  });
  const [shortsCfg, setShortsCfg] = useState<ShortsConfig>(() => {
    try {
      const saved = localStorage.getItem("shorts");
      if (saved) return { ...DEFAULT_SHORTS, ...JSON.parse(saved) };
    } catch {}
    return DEFAULT_SHORTS;
  });

  useEffect(() => {
    localStorage.setItem("shorts", JSON.stringify(shortsCfg));
  }, [shortsCfg]);

  // Check whether yt-dlp / ffmpeg / ffprobe are installed in AppLocalData
  useEffect(() => {
    (async () => {
      try {
        const ready = await invoke<boolean>("check_binaries_ready");
        setBinariesReady(ready);
      } catch {
        setBinariesReady(false);
      }
    })();
  }, []);

  // Listen to setup-progress events from setup_binaries
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    listen<SetupProgress>("setup-progress", (e) => {
      setSetupProgress(e.payload);
    }).then((fn) => (unlisten = fn));
    return () => {
      unlisten?.();
    };
  }, []);

  const onSetupStart = async () => {
    setSetupRunning(true);
    setSetupError(null);
    setSetupProgress({
      step: "init",
      percent: 0,
      downloaded_mb: 0,
      total_mb: 0,
      message: "준비 중...",
    });
    try {
      await invoke<string>("setup_binaries");
      setBinariesReady(true);
    } catch (e) {
      setSetupError(String(e));
    } finally {
      setSetupRunning(false);
    }
  };

  const updateShorts = <K extends keyof ShortsConfig>(k: K, v: ShortsConfig[K]) =>
    setShortsCfg((prev) => ({ ...prev, [k]: v }));

  useEffect(() => {
    localStorage.setItem("adv", JSON.stringify(adv));
  }, [adv]);

  useEffect(() => {
    localStorage.setItem("advOpen", advOpen ? "1" : "0");
  }, [advOpen]);

  const updateAdv = <K extends keyof AdvancedOpts>(k: K, v: AdvancedOpts[K]) =>
    setAdv((prev) => ({ ...prev, [k]: v }));

  const advActiveCount =
    (adv.subtitlesEnabled ? 1 : 0) +
    (adv.sponsorblock ? 1 : 0) +
    (adv.rateLimit ? 1 : 0) +
    (adv.cookiesBrowser ? 1 : 0) +
    (adv.splitChapters ? 1 : 0) +
    (adv.sectionEnabled ? 1 : 0) +
    (adv.liveFromStart ? 1 : 0);

  const [library, setLibrary] = useState<LibraryItem[]>(() => {
    try {
      const saved = localStorage.getItem("library");
      if (saved) return JSON.parse(saved);
    } catch {}
    return [];
  });
  const [libOpen, setLibOpen] = useState<boolean>(
    () => localStorage.getItem("libOpen") === "1"
  );
  useEffect(() => {
    localStorage.setItem("libOpen", libOpen ? "1" : "0");
  }, [libOpen]);

  const addToLibrary = (item: QueueItem) => {
    setLibrary((prev) => {
      const newItem: LibraryItem = {
        id: item.id,
        title: item.info.title,
        thumbnail: item.info.thumbnail,
        formatLabel: item.formatLabel,
        outputDir: item.outputDir,
        completedAt: Date.now(),
      };
      const next = [newItem, ...prev.filter((p) => p.id !== item.id)].slice(0, 30);
      localStorage.setItem("library", JSON.stringify(next));
      return next;
    });
  };

  const onClearLibrary = () => {
    if (confirm("라이브러리를 비우시겠습니까?")) {
      setLibrary([]);
      localStorage.removeItem("library");
    }
  };

  const onRemoveLibraryItem = (id: string, completedAt: number) => {
    setLibrary((prev) => {
      const next = prev.filter(
        (p) => !(p.id === id && p.completedAt === completedAt)
      );
      localStorage.setItem("library", JSON.stringify(next));
      return next;
    });
  };

  const onOpenLibFolder = async (path: string) => {
    try {
      await invoke("open_folder", { path });
    } catch (e) {
      setError(String(e));
    }
  };

  const urlRef = useRef(url);
  urlRef.current = url;
  const processingRef = useRef<string | null>(null);

  const formatId = useMemo(() => {
    if (formatType === "mp3") return "mp3";
    if (formatType === "webm") return "webm";
    if (formatType === "premiere") return `premiere-${quality}`;
    return `mp4-${quality}`;
  }, [formatType, quality]);

  const isDownloading = queue.some((q) => q.status === "downloading");

  // Try to auto-paste a YouTube URL from clipboard if it's different from current.
  // Extracts only the URL substring so we don't dump arbitrary text (e.g. logs) into the input.
  const tryAutoPasteClipboard = async () => {
    try {
      const txt = await readText();
      if (!txt) return;
      const match = txt.match(VIDEO_URL_EXTRACT);
      if (!match) return;
      const extracted = match[0];
      if (extracted !== urlRef.current.trim()) {
        setUrl(extracted);
        setError(null);
      }
    } catch {
      /* clipboard empty or denied */
    }
  };

  // Initial clipboard check + window-focus listener
  useEffect(() => {
    tryAutoPasteClipboard();
    const onFocus = () => tryAutoPasteClipboard();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // Auto-detect user's Downloads folder on first run (no saved value)
  useEffect(() => {
    if (outputDir) return;
    downloadDir()
      .then((dir) => {
        if (dir) {
          setOutputDir(dir);
          localStorage.setItem("outputDir", dir);
        }
      })
      .catch(() => {
        /* path API failed — user can set manually */
      });
  }, []);

  // Request notification permission once
  useEffect(() => {
    (async () => {
      try {
        const granted = await isPermissionGranted();
        if (!granted) await requestPermission();
      } catch {
        /* skip */
      }
    })();
  }, []);

  // Debounced auto-fetch info when URL changes (gated on binaries being ready)
  useEffect(() => {
    if (!binariesReady) return;
    const trimmed = url.trim();
    if (!trimmed || !ANY_URL_RE.test(trimmed)) {
      setInfo(null);
      return;
    }
    setLoadingInfo(true);
    setError(null);
    const t = setTimeout(async () => {
      try {
        const data = await invoke<VideoInfo>("get_video_info", { url: normalizeUrl(trimmed) });
        setInfo(data);
      } catch (e) {
        setError(String(e));
        setInfo(null);
      } finally {
        setLoadingInfo(false);
      }
    }, 500);
    return () => {
      clearTimeout(t);
      setLoadingInfo(false);
    };
  }, [url, binariesReady]);

  // Clear leftover error from before binaries finished installing
  useEffect(() => {
    if (binariesReady) setError(null);
  }, [binariesReady]);

  // Check yt-dlp update on mount (gated on binaries; silent fail if offline)
  useEffect(() => {
    if (!binariesReady) return;
    (async () => {
      try {
        const status = await invoke<UpdateStatus>("check_yt_dlp_update");
        setUpdateStatus(status);
      } catch {
        /* offline or rate-limited */
      }
    })();
  }, [binariesReady]);

  // Check app distribution update (mkvibe.com version.json; silent fail if offline)
  // 바이너리 설치 여부와 무관하게 mount 시 바로 확인 — 셋업이 깨진 사용자도 앱 업데이트는 봐야 함
  useEffect(() => {
    (async () => {
      try {
        const status = await invoke<AppUpdateStatus>("check_app_update");
        setAppUpdate(status);
      } catch {
        /* offline */
      }
    })();
  }, []);

  // Fetch announcement from mkvibe.com (gated on binaries; silent fail if offline)
  useEffect(() => {
    if (!binariesReady) return;
    (async () => {
      try {
        const a = await invoke<Announcement | null>("fetch_announcement");
        if (!a || !a.id) return;
        const now = new Date();
        if (a.show_from && new Date(a.show_from) > now) return;
        if (a.show_until && new Date(a.show_until) < now) return;
        const dismissed: string[] = JSON.parse(
          localStorage.getItem("dismissedAnnouncements") || "[]"
        );
        if (dismissed.includes(a.id)) return;
        setAnnouncement(a);
      } catch {
        /* offline */
      }
    })();
  }, [binariesReady]);

  const dismissAnnouncement = () => {
    if (announcement) {
      const dismissed: string[] = JSON.parse(
        localStorage.getItem("dismissedAnnouncements") || "[]"
      );
      if (!dismissed.includes(announcement.id)) {
        dismissed.push(announcement.id);
        localStorage.setItem("dismissedAnnouncements", JSON.stringify(dismissed));
      }
    }
    setAnnouncement(null);
  };

  const onClickAnnouncement = async () => {
    if (announcement?.link_url) {
      try {
        await invoke("open_url", { url: announcement.link_url });
      } catch (e) {
        setError(String(e));
      }
    }
    dismissAnnouncement();
  };

  // Listen to update-progress events
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    listen<string>("update-progress", (e) => setUpdateMsg(e.payload)).then(
      (fn) => (unlisten = fn)
    );
    return () => {
      unlisten?.();
    };
  }, []);

  // Listen to download progress events and update the active queue item
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    listen<Progress>("download-progress", (e) => {
      setQueue((prev) =>
        prev.map((q) =>
          q.status === "downloading" ? { ...q, progress: e.payload } : q
        )
      );
    }).then((fn) => (unlisten = fn));
    return () => {
      unlisten?.();
    };
  }, []);

  // Queue processor: start the next pending item when nothing is downloading
  useEffect(() => {
    if (isDownloading) return;
    const next = queue.find((q) => q.status === "pending");
    if (!next) return;
    if (processingRef.current === next.id) return;
    processingRef.current = next.id;

    setQueue((prev) =>
      prev.map((q) => (q.id === next.id ? { ...q, status: "downloading" } : q))
    );

    const downloadPromise = next.shorts
      ? invoke("generate_shorts", {
          opts: {
            url: next.url,
            output_dir: next.outputDir,
            shorts_count: next.shorts.count,
            clip_duration: next.shorts.duration,
            crop_mode: next.shorts.cropMode,
            cookies_browser: next.adv.cookiesBrowser,
          },
        })
      : invoke("download_video", {
          opts: {
            url: next.url,
            format_id: next.formatId,
            output_dir: next.outputDir,
            subtitles_enabled: next.adv.subtitlesEnabled,
            subtitle_langs: next.adv.subtitleLangs,
            subtitle_auto: next.adv.subtitleAuto,
            // Premiere(ProRes MOV)는 임베드가 변환에서 유실되므로 외부 SRT로만 저장
            subtitle_embed: next.formatId.startsWith("premiere-")
              ? false
              : next.adv.subtitleEmbed,
            // Premiere는 다중 파일 변환 미지원이라 챕터 분할 차단 (UI에서도 비활성)
            split_chapters: next.formatId.startsWith("premiere-")
              ? false
              : next.adv.splitChapters,
            sponsorblock: next.adv.sponsorblock,
            rate_limit: next.adv.rateLimit,
            cookies_browser: next.adv.cookiesBrowser,
            download_section: next.adv.sectionEnabled ? next.adv.downloadSection : "",
            live_from_start: next.adv.liveFromStart,
            gif_fps: next.adv.gifFps,
            gif_width: next.adv.gifWidth,
            duration_seconds: next.info.duration || 0,
          },
        });

    downloadPromise
      .then(() => {
        setQueue((prev) =>
          prev.map((q) =>
            q.id === next.id
              ? {
                  ...q,
                  status: "completed",
                  progress: {
                    percent: 100,
                    speed: "",
                    eta: "",
                    total_size: "",
                    status: "finished",
                    message: "완료",
                  },
                }
              : q
          )
        );
        addToLibrary(next);
        try {
          sendNotification({
            title: "다운로드 완료",
            body: next.info.title,
          });
        } catch {
          /* notification errors are non-critical */
        }
      })
      .catch((e) => {
        const wasCancelled = cancelledIdsRef.current.has(next.id);
        cancelledIdsRef.current.delete(next.id);
        setQueue((prev) =>
          prev.map((q) =>
            q.id === next.id
              ? {
                  ...q,
                  status: "failed",
                  error: wasCancelled ? "사용자가 취소함" : String(e),
                }
              : q
          )
        );
      })
      .finally(() => {
        processingRef.current = null;
      });
  }, [queue, isDownloading]);

  const onPickFolder = async () => {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      defaultPath: outputDir || undefined,
    });
    if (typeof selected === "string") {
      setOutputDir(selected);
      localStorage.setItem("outputDir", selected);
    }
  };

  const onAddToQueue = () => {
    if (!url.trim()) return setError("URL을 입력해주세요");
    if (!outputDir) return setError("저장 폴더를 선택해주세요");
    if (!info) return setError("영상 정보를 먼저 불러오세요");

    const item: QueueItem = {
      id: genId(),
      url: normalizeUrl(url),
      info,
      formatLabel: formatLabelFor(formatType, quality, shortsCfg),
      formatId,
      outputDir,
      adv: { ...adv },
      shorts: formatType === "shorts" ? { ...shortsCfg } : undefined,
      status: "pending",
    };
    setQueue((prev) => [...prev, item]);
    setUrl("");
    setInfo(null);
    setError(null);
  };

  const onRemoveItem = (id: string) => {
    setQueue((prev) => prev.filter((q) => q.id !== id));
  };

  const cancelledIdsRef = useRef<Set<string>>(new Set());

  const onCancelItem = async (id: string) => {
    if (!confirm("진행 중인 다운로드를 취소하시겠습니까?\n받던 임시 파일은 자동 정리됩니다.")) return;
    cancelledIdsRef.current.add(id);
    try {
      await invoke<boolean>("cancel_download");
    } catch (e) {
      setError(String(e));
    }
  };

  const onRetryItem = (id: string) => {
    setQueue((prev) =>
      prev.map((q) =>
        q.id === id ? { ...q, status: "pending", error: undefined, progress: undefined } : q
      )
    );
  };

  const onClearCompleted = () => {
    setQueue((prev) => prev.filter((q) => q.status !== "completed" && q.status !== "failed"));
  };

  const onOpenItemFolder = async (item: QueueItem) => {
    try {
      await invoke("open_folder", { path: item.outputDir });
    } catch (e) {
      setError(String(e));
    }
  };

  const onOpenFolder = async () => {
    if (outputDir) {
      try {
        await invoke("open_folder", { path: outputDir });
      } catch (e) {
        setError(String(e));
      }
    }
  };

  const onUpdateApp = async () => {
    if (isDownloading) {
      setError("다운로드 중에는 업데이트할 수 없습니다");
      return;
    }
    setAppUpdating(true);
    setUpdateMsg("준비 중...");
    setError(null);
    try {
      // 성공하면 설치기가 뜨고 앱은 스스로 종료된다
      await invoke("install_app_update");
    } catch (e) {
      setError(String(e));
      setAppUpdating(false);
      setUpdateMsg("");
    }
  };

  const onUpdateYtDlp = async () => {
    if (isDownloading) {
      setError("다운로드 중에는 업데이트할 수 없습니다");
      return;
    }
    setUpdating(true);
    setUpdateMsg("준비 중...");
    setError(null);
    try {
      const newVersion = await invoke<string>("update_yt_dlp");
      setUpdateDone(newVersion);
      setUpdateStatus({
        current_version: newVersion,
        latest_version: newVersion,
        update_available: false,
      });
      setUpdateMsg("");
    } catch (e) {
      setError(String(e));
      setUpdateMsg("");
    } finally {
      setUpdating(false);
    }
  };

  // Drag & drop handlers
  const onDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    if (!dragOver) setDragOver(true);
  };
  const onDragLeave = (e: React.DragEvent) => {
    if (e.currentTarget === e.target) setDragOver(false);
  };
  const onDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    const text =
      e.dataTransfer.getData("text/uri-list") ||
      e.dataTransfer.getData("text/plain");
    if (!text) return;
    const match = text.match(VIDEO_URL_EXTRACT);
    if (match) {
      setUrl(match[0]);
      setError(null);
    } else {
      setError("지원하는 동영상 링크가 아닙니다");
    }
  };

  const completedCount = queue.filter((q) => q.status === "completed").length;
  const finishedCount = queue.filter(
    (q) => q.status === "completed" || q.status === "failed"
  ).length;

  if (binariesReady === null) {
    return (
      <div className="app">
        <div className="setup-loading">
          <div className="logo-mark">▼</div>
          <p>확인 중...</p>
        </div>
      </div>
    );
  }

  if (binariesReady === false) {
    return (
      <div className="app">
        <header className="header">
          <div className="logo-mark">▼</div>
          <h1>MK 유튜브 다운로더</h1>
          <span className="edition-badge edition-basic" title="Basic 에디션 v1.0.2">
            BASIC <b>v1.0.2</b>
          </span>
        </header>
        {appUpdate?.update_available && (
          <section className="update-banner">
            <div className="update-info">
              <span className="update-icon">🚀</span>
              <div>
                <div className="update-title">앱 새 버전 업데이트 가능</div>
                <div className="update-sub">
                  v{appUpdate.current_version} → <b>v{appUpdate.latest_version}</b>
                </div>
              </div>
            </div>
            <button
              className="btn btn-update"
              onClick={onUpdateApp}
              disabled={appUpdating}
            >
              {appUpdating ? updateMsg || "업데이트 중..." : "업데이트"}
            </button>
          </section>
        )}
        <section className="card setup-card">
          <h2 className="setup-title">🎬 처음 실행이네요</h2>
          <p className="setup-desc">
            유튜브 다운로드에 필요한 도구를 한 번만 받으면 됩니다. 약 <b>230MB</b>.
          </p>
          <div className="setup-list">
            <div className="setup-item">
              <span className="setup-bullet">📦</span>
              <div>
                <b>다운로드 엔진</b>
                <span className="setup-sub"> · 동영상 추출 모듈 · 18 MB · SHA-256 검증</span>
              </div>
            </div>
            <div className="setup-item">
              <span className="setup-bullet">🎞</span>
              <div>
                <b>ffmpeg + ffprobe</b>
                <span className="setup-sub"> · 영상 처리 · 210 MB</span>
              </div>
            </div>
          </div>
          <div className="setup-meta">
            <div>📍 설치 경로: <code>%LocalAppData%\com.megakim.youtubedownloader\binaries</code></div>
            <div>🌐 검증된 공식 오픈소스 릴리즈에서만 다운로드</div>
          </div>

          {setupRunning && setupProgress && (
            <div className="setup-progress">
              <div className="progress-bar setup-bar">
                <div
                  key={setupProgress.step}
                  className="progress-fill"
                  style={{ width: `${setupProgress.percent}%` }}
                />
              </div>
              <div className="progress-info">
                <span className="pct">{setupProgress.percent.toFixed(1)}%</span>
                <span className="status">{setupProgress.message}</span>
              </div>
            </div>
          )}

          {setupError && (
            <div className="error">⚠ {setupError}</div>
          )}

          {!setupRunning && (
            <button className="btn btn-download" onClick={onSetupStart}>
              {setupError ? "↻ 다시 시도" : "⬇ 다운로드 시작"}
            </button>
          )}

          <p className="setup-note">
            다운로드 시간 약 5~20초 걸립니다
          </p>
        </section>
      </div>
    );
  }

  return (
    <div
      className={`app ${dragOver ? "drag-over" : ""}`}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
    >
      <header className="header">
        <div className="logo-mark">▼</div>
        <h1>MK 유튜브 다운로더</h1>
        <span className="edition-badge edition-basic" title="Basic 에디션 v1.0.2">
          BASIC <b>v1.0.2</b>
        </span>
        <a
          className="brand-mkvibe-link"
          onClick={(e) => {
            e.preventDefault();
            invoke("open_url", { url: "https://mkvibe.com" }).catch(() => {});
          }}
          title="mkvibe.com — 메가킴 바이브 코딩 스튜디오"
          href="https://mkvibe.com"
        >
          <span className="brand-mkvibe">mkvibe.com</span>
        </a>
      </header>

      {appUpdate?.update_available && (
        <section className="update-banner">
          <div className="update-info">
            <span className="update-icon">🚀</span>
            <div>
              <div className="update-title">앱 새 버전 업데이트 가능</div>
              <div className="update-sub">
                v{appUpdate.current_version} → <b>v{appUpdate.latest_version}</b>
                {updateStatus?.update_available && (
                  <span> · 엔진 업데이트는 앱 업데이트 후 진행돼요</span>
                )}
              </div>
            </div>
          </div>
          <button
            className="btn btn-update"
            onClick={onUpdateApp}
            disabled={appUpdating || isDownloading}
          >
            {appUpdating ? updateMsg || "업데이트 중..." : "업데이트"}
          </button>
        </section>
      )}

      {!appUpdate?.update_available && updateStatus?.update_available && !updateDone && (
        <section className="update-banner">
          <div className="update-info">
            <span className="update-icon">🔔</span>
            <div>
              <div className="update-title">다운로드 엔진 업데이트 가능</div>
              <div className="update-sub">
                {updateStatus.current_version} → <b>{updateStatus.latest_version}</b>
              </div>
            </div>
          </div>
          <button
            className="btn btn-update"
            onClick={onUpdateYtDlp}
            disabled={updating || isDownloading}
          >
            {updating ? updateMsg || "업데이트 중..." : "업데이트"}
          </button>
        </section>
      )}

      {updateDone && (
        <section className="update-banner success">
          <div className="update-info">
            <span className="update-icon">✅</span>
            <div>
              <div className="update-title">업데이트 완료</div>
              <div className="update-sub">현재 버전: <b>{updateDone}</b></div>
            </div>
          </div>
          <button className="btn btn-ghost" onClick={() => setUpdateDone(null)}>
            닫기
          </button>
        </section>
      )}

      <section className="card">
        <label className="label">
          동영상 주소
          {loadingInfo && <span className="hint">정보 불러오는 중...</span>}
        </label>
        <input
          type="text"
          className="input url-input"
          placeholder="링크를 복사하거나 끌어다 놓으세요"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
        <div className="supported-sites">
          <span className="ss-label">지원</span>
          <span className="ss-chip yt"><i></i>YouTube</span>
          <span className="ss-chip soop"><i></i>SOOP</span>
          <span className="ss-chip twitch"><i></i>Twitch</span>
          <span className="ss-chip tiktok"><i></i>TikTok</span>
          <span className="ss-chip instagram"><i></i>Instagram</span>
          <span className="ss-chip x"><i></i>X</span>
          <span className="ss-chip naver"><i></i>네이버TV</span>
          <span className="ss-chip kakao"><i></i>카카오TV</span>
          <span className="ss-chip vimeo"><i></i>Vimeo</span>
          <span className="ss-chip more">+1700개</span>
        </div>
      </section>

      {info && (
        <section className="card">
          <label className="label">영상정보</label>
          <div className="preview">
            {info.thumbnail && (
              <img src={info.thumbnail} alt="" className="thumb" />
            )}
            <div className="meta">
              <h2 className="title">{info.title}</h2>
              <div className="sub">
                <span>👤 {info.uploader || "-"}</span>
                <span>⏱ {formatDuration(info.duration)}</span>
                <span>👁 {formatViews(info.view_count)}</span>
              </div>
              <div className="quality-tags">
                {info.is_live && <span className="qtag live">🔴 LIVE</span>}
                {info.chapter_count > 0 && (
                  <span className="qtag chapter">📑 챕터 {info.chapter_count}개</span>
                )}
                <span className="qtag video">🎬 최대 {formatMaxVideo(info)}</span>
                <span className="qtag audio">🎵 최대 {formatMaxAudio(info)}</span>
              </div>
            </div>
          </div>
        </section>
      )}

      <section className="card">
        <label className="label">형식</label>
        <div className="seg">
          {(["mp4", "premiere", "mp3", "webm", "gif", "shorts"] as FormatType[]).map((t) => (
            <button
              key={t}
              className={`seg-btn ${formatType === t ? "active" : ""} ${t === "shorts" ? "seg-shorts" : ""}`}
              onClick={() => setFormatType(t)}
            >
              {t === "mp4"
                ? "🎬 MP4 영상"
                : t === "premiere"
                ? "✂️ Premiere 편집용"
                : t === "mp3"
                ? "🎵 MP3 음원"
                : t === "webm"
                ? "🌐 WebM"
                : t === "gif"
                ? "🎞 GIF 움짤"
                : "✨ 쇼츠 자동"}
            </button>
          ))}
        </div>
        {formatType === "premiere" && (
          <div className="adv-desc" style={{ marginTop: 10 }}>
            ✂️ 다운로드 후 프리미어 프로가 지원하는 <b>MOV (ProRes 422 HQ)</b>로 자동 변환합니다
            — 화질 보존 · 용량 큼. 자막은 임베드 대신 SRT 파일로 따로 저장돼요.
          </div>
        )}
        {formatType === "gif" && (
          <div className="adv-sub" style={{ marginLeft: 0, marginTop: 12 }}>
            <div className="adv-field">
              <span className="adv-field-label">FPS</span>
              <input
                type="number"
                className="input input-sm"
                value={adv.gifFps}
                min={5}
                max={30}
                onChange={(e) => updateAdv("gifFps", Number(e.target.value) || 15)}
                style={{ width: 80 }}
              />
              <span className="adv-field-label">너비(px)</span>
              <input
                type="number"
                className="input input-sm"
                value={adv.gifWidth}
                min={120}
                max={1280}
                step={20}
                onChange={(e) => updateAdv("gifWidth", Number(e.target.value) || 480)}
                style={{ width: 100 }}
              />
              <span className="adv-desc" style={{ marginLeft: "auto" }}>
                💡 구간 자르기와 함께 쓰면 좋아요
              </span>
            </div>
          </div>
        )}
      </section>

      {(formatType === "mp4" || formatType === "premiere") && (
        <section className="card">
          <label className="label">화질</label>
          <div className="seg">
            {(["best", "2160", "1440", "1080", "720", "480"] as Quality[]).map(
              (q) => (
                <button
                  key={q}
                  className={`seg-btn ${quality === q ? "active" : ""}`}
                  onClick={() => setQuality(q)}
                >
                  {q === "best" ? "최고" : q === "2160" ? "4K" : `${q}p`}
                </button>
              )
            )}
          </div>
        </section>
      )}

      {formatType === "shorts" && (
        <section className="card">
          <label className="label">
            ✨ 쇼츠 설정
            <span className="hint" style={{ fontWeight: 500 }}>
              유튜브 "다시 본 구간" 데이터로 하이라이트 자동 추출
            </span>
          </label>

          <div className="adv-row" style={{ paddingTop: 6 }}>
            <div className="adv-field">
              <span className="adv-field-label">
                <b>생성 개수</b>
                <span className="adv-desc">한 영상에서 만들 쇼츠 수</span>
              </span>
              <div className="seg" style={{ flex: "0 0 auto" }}>
                {[1, 3, 5, 7, 10].map((n) => (
                  <button
                    key={n}
                    className={`seg-btn ${shortsCfg.count === n ? "active" : ""}`}
                    onClick={() => updateShorts("count", n)}
                    style={{ minWidth: 44 }}
                  >
                    {n}개
                  </button>
                ))}
              </div>
            </div>
          </div>

          <div className="adv-row">
            <div className="adv-field">
              <span className="adv-field-label">
                <b>각 쇼츠 길이</b>
                <span className="adv-desc">하이라이트 지점 기준 좌우</span>
              </span>
              <div className="seg" style={{ flex: "0 0 auto" }}>
                {[15, 30, 45, 60].map((d) => (
                  <button
                    key={d}
                    className={`seg-btn ${shortsCfg.duration === d ? "active" : ""}`}
                    onClick={() => updateShorts("duration", d)}
                    style={{ minWidth: 54 }}
                  >
                    {d}초
                  </button>
                ))}
              </div>
            </div>
          </div>

          <div className="adv-row">
            <div className="adv-field">
              <span className="adv-field-label">
                <b>세로 9:16 변환</b>
                <span className="adv-desc">
                  {shortsCfg.cropMode === "center"
                    ? "양옆 잘림 (얼굴 중앙에 있을 때 좋음)"
                    : "원본 유지 + 위아래 블러 배경 (양옆 안 잘림)"}
                </span>
              </span>
              <div className="seg" style={{ flex: "0 0 auto" }}>
                <button
                  className={`seg-btn ${shortsCfg.cropMode === "center" ? "active" : ""}`}
                  onClick={() => updateShorts("cropMode", "center")}
                >
                  가운데 크롭
                </button>
                <button
                  className={`seg-btn ${shortsCfg.cropMode === "blur" ? "active" : ""}`}
                  onClick={() => updateShorts("cropMode", "blur")}
                >
                  블러 패딩
                </button>
              </div>
            </div>
          </div>

          {info && info.duration > 0 && (
            <div className="adv-row">
              <div className="shorts-estimate">
                💾 예상 처리 시간: 약 {Math.ceil((shortsCfg.count * shortsCfg.duration) / 30)}분
                · 추출+변환 포함
              </div>
            </div>
          )}
        </section>
      )}

      <section className="card adv-card">
        <button
          className="adv-toggle"
          onClick={() => setAdvOpen((v) => !v)}
        >
          <span className="label" style={{ margin: 0 }}>
            ⚙ 고급 설정
            {advActiveCount > 0 && (
              <span className="adv-count">{advActiveCount}개 활성</span>
            )}
          </span>
          <span className={`chevron ${advOpen ? "open" : ""}`}>▼</span>
        </button>

        {advOpen && (
          <div className="adv-body">
            <div className="adv-row">
              <label className="check-row">
                <span className={`check ${adv.subtitlesEnabled ? "on" : ""}`}>
                  {adv.subtitlesEnabled && "✓"}
                </span>
                <span>
                  <b>자막 다운로드</b>
                  <span className="adv-desc">SRT 파일로 함께 저장 (MP3 제외)</span>
                </span>
                <input
                  type="checkbox"
                  checked={adv.subtitlesEnabled}
                  onChange={(e) => updateAdv("subtitlesEnabled", e.target.checked)}
                  hidden
                />
              </label>
              {adv.subtitlesEnabled && (
                <div className="adv-sub">
                  <div className="adv-field">
                    <span className="adv-field-label">언어</span>
                    <input
                      type="text"
                      className="input input-sm"
                      value={adv.subtitleLangs}
                      placeholder="ko,en,ja"
                      onChange={(e) => updateAdv("subtitleLangs", e.target.value)}
                    />
                  </div>
                  <label className="check-row mini">
                    <span className={`check ${adv.subtitleAuto ? "on" : ""}`}>
                      {adv.subtitleAuto && "✓"}
                    </span>
                    <span>자동 생성 자막 포함 (CC)</span>
                    <input type="checkbox" checked={adv.subtitleAuto}
                      onChange={(e) => updateAdv("subtitleAuto", e.target.checked)} hidden />
                  </label>
                  <label className="check-row mini">
                    <span className={`check ${adv.subtitleEmbed ? "on" : ""}`}>
                      {adv.subtitleEmbed && "✓"}
                    </span>
                    <span>영상 파일에 자막 임베드</span>
                    <input type="checkbox" checked={adv.subtitleEmbed}
                      onChange={(e) => updateAdv("subtitleEmbed", e.target.checked)} hidden />
                  </label>
                </div>
              )}
            </div>

            <div className="adv-row">
              <label className="check-row">
                <span className={`check ${adv.sponsorblock ? "on" : ""}`}>
                  {adv.sponsorblock && "✓"}
                </span>
                <span>
                  <b>SponsorBlock</b>
                  <span className="adv-desc">광고/스폰서/인트로/아웃트로 자동 제거</span>
                </span>
                <input type="checkbox" checked={adv.sponsorblock}
                  onChange={(e) => updateAdv("sponsorblock", e.target.checked)} hidden />
              </label>
            </div>

            <div className="adv-row" style={formatType === "premiere" ? { opacity: 0.45 } : undefined}>
              <label className="check-row">
                <span className={`check ${formatType !== "premiere" && adv.splitChapters ? "on" : ""}`}>
                  {formatType !== "premiere" && adv.splitChapters && "✓"}
                </span>
                <span>
                  <b>챕터별 분할</b>
                  <span className="adv-desc">
                    {formatType === "premiere"
                      ? "Premiere 편집용 형식에서는 쓸 수 없어요 (여러 파일 변환 미지원)"
                      : "영상의 챕터마다 별도 파일로 저장"}
                  </span>
                </span>
                <input type="checkbox"
                  checked={formatType !== "premiere" && adv.splitChapters}
                  disabled={formatType === "premiere"}
                  onChange={(e) => updateAdv("splitChapters", e.target.checked)} hidden />
              </label>
            </div>

            <div className="adv-row">
              <label className="check-row">
                <span className={`check ${adv.sectionEnabled ? "on" : ""}`}>
                  {adv.sectionEnabled && "✓"}
                </span>
                <span>
                  <b>구간 자르기</b>
                  <span className="adv-desc">시:분:초 지정 구간만 다운로드 · 시작점은 가장 가까운 키프레임</span>
                </span>
                <input
                  type="checkbox"
                  checked={adv.sectionEnabled}
                  onChange={(e) => updateAdv("sectionEnabled", e.target.checked)}
                  hidden
                />
              </label>
              {adv.sectionEnabled && (
                <div className="adv-sub">
                  {(() => {
                    const t = parseSectionTime(adv.downloadSection);
                    const setT = (next: SectionTime) =>
                      updateAdv("downloadSection", formatSectionTime(next));
                    return (
                      <div className="time-row">
                        <span className="time-tag">시작</span>
                        <TimeUnit value={t.sH} max={23} onChange={(n) => setT({ ...t, sH: n })} />
                        <span className="time-sep">:</span>
                        <TimeUnit value={t.sM} max={59} onChange={(n) => setT({ ...t, sM: n })} />
                        <span className="time-sep">:</span>
                        <TimeUnit value={t.sS} max={59} onChange={(n) => setT({ ...t, sS: n })} />
                        <span className="time-tilde">~</span>
                        <span className="time-tag">끝</span>
                        <TimeUnit value={t.eH} max={23} onChange={(n) => setT({ ...t, eH: n })} />
                        <span className="time-sep">:</span>
                        <TimeUnit value={t.eM} max={59} onChange={(n) => setT({ ...t, eM: n })} />
                        <span className="time-sep">:</span>
                        <TimeUnit value={t.eS} max={59} onChange={(n) => setT({ ...t, eS: n })} />
                      </div>
                    );
                  })()}
                </div>
              )}
            </div>

            {info?.is_live && (
              <div className="adv-row">
                <label className="check-row">
                  <span className={`check ${adv.liveFromStart ? "on" : ""}`}>
                    {adv.liveFromStart && "✓"}
                  </span>
                  <span>
                    <b>라이브 처음부터 녹화</b>
                    <span className="adv-desc">
                      현재 시점이 아닌 방송 시작부터 다운로드 (실험적)
                    </span>
                  </span>
                  <input type="checkbox" checked={adv.liveFromStart}
                    onChange={(e) => updateAdv("liveFromStart", e.target.checked)} hidden />
                </label>
              </div>
            )}

            <div className="adv-row">
              <div className="adv-field">
                <span className="adv-field-label">
                  <b>속도 제한</b>
                  <span className="adv-desc">MB/s · 빈칸이면 무제한</span>
                </span>
                <input
                  type="text"
                  className="input input-sm"
                  value={adv.rateLimit}
                  placeholder="예: 5"
                  onChange={(e) => updateAdv("rateLimit", e.target.value)}
                  style={{ width: 100 }}
                />
              </div>
            </div>

            <div className="adv-row">
              <div className="adv-field">
                <span className="adv-field-label">
                  <b>브라우저 쿠키</b>
                  <span className="adv-desc">멤버십/연령제한 영상용 (브라우저 닫고 사용 권장)</span>
                </span>
                <select
                  className="input input-sm"
                  value={adv.cookiesBrowser}
                  onChange={(e) => updateAdv("cookiesBrowser", e.target.value)}
                  style={{ width: 140 }}
                >
                  <option value="">사용 안 함</option>
                  <option value="chrome">Chrome</option>
                  <option value="edge">Edge</option>
                  <option value="firefox">Firefox</option>
                  <option value="brave">Brave</option>
                  <option value="whale">Whale</option>
                </select>
              </div>
            </div>
          </div>
        )}
      </section>

      <section className="card">
        <label className="label">저장폴더</label>
        <div className="row">
          <button className="btn btn-ghost" onClick={onPickFolder}>
            📁 저장위치
          </button>
          <input
            type="text"
            className="input"
            value={outputDir}
            placeholder="폴더를 선택해주세요"
            readOnly
          />
          {outputDir && (
            <button
              className="btn btn-ghost"
              onClick={onOpenFolder}
              title="다운로드 폴더 열기"
            >
              📂 저장폴더
            </button>
          )}
        </div>
      </section>

      {error && <div className="error">⚠ {error}</div>}

      <button
        className="btn btn-download"
        onClick={onAddToQueue}
        disabled={!url || !outputDir || !info || loadingInfo}
      >
        {isDownloading ? "⊕ 큐에 추가" : "⬇ 다운로드 시작"}
      </button>

      {queue.length > 0 && (
        <section className="card queue-card">
          <div className="queue-header">
            <label className="label" style={{ margin: 0 }}>
              다운로드 큐 <span className="queue-count">{queue.length}</span>
            </label>
            {finishedCount > 0 && (
              <button className="btn btn-ghost btn-sm" onClick={onClearCompleted}>
                완료 항목 비우기
              </button>
            )}
          </div>
          <div className="queue-list">
            {queue.map((item) => (
              <div key={item.id} className={`queue-item q-${item.status}`}>
                {item.info.thumbnail && (
                  <img src={item.info.thumbnail} alt="" className="qthumb" />
                )}
                <div className="qmeta">
                  <div className="qtitle">{item.info.title}</div>
                  <div className="qbadges">
                    <span className="qbadge">{item.formatLabel}</span>
                    {item.status === "pending" && (
                      <span className="qbadge pending">⏸ 대기 중</span>
                    )}
                    {item.status === "downloading" && (
                      item.progress ? (
                        <>
                          {item.progress.total_parts && item.progress.total_parts > 1 ? (
                            <span className="qbadge part">
                              📑 파트 {item.progress.current_part}/{item.progress.total_parts}
                            </span>
                          ) : null}
                          {item.progress.status === "downloading" ? (
                            <span className="qbadge downloading">
                              ⬇ {item.progress.percent.toFixed(1)}% · {item.progress.speed || "..."}
                            </span>
                          ) : (
                            <span className="qbadge downloading">
                              🔄 {item.progress.message || item.progress.status}
                            </span>
                          )}
                        </>
                      ) : (
                        <span className="qbadge downloading">⬇ 시작 중...</span>
                      )
                    )}
                    {item.status === "completed" && (
                      <span className="qbadge completed">✓ 완료</span>
                    )}
                    {item.status === "failed" && (
                      <span className="qbadge failed">⚠ 실패</span>
                    )}
                  </div>
                  {item.status === "downloading" && item.progress && (
                    <div className="qbar">
                      <div
                        key={item.progress.current_part || 0}
                        className="qbar-fill"
                        style={{ width: `${item.progress.percent}%` }}
                      />
                    </div>
                  )}
                  {item.status === "failed" && item.error && (
                    <div className="qerror">{item.error}</div>
                  )}
                </div>
                <div className="qactions">
                  {item.status === "completed" && (
                    <button
                      className="btn btn-ghost btn-icon"
                      onClick={() => onOpenItemFolder(item)}
                      title="폴더 열기"
                    >
                      📂
                    </button>
                  )}
                  {item.status === "failed" && (
                    <button
                      className="btn btn-ghost btn-icon"
                      onClick={() => onRetryItem(item.id)}
                      title="재시도"
                    >
                      🔄
                    </button>
                  )}
                  {item.status === "downloading" && (
                    <button
                      className="btn btn-ghost btn-icon btn-cancel"
                      onClick={() => onCancelItem(item.id)}
                      title="다운로드 취소"
                    >
                      ⏹
                    </button>
                  )}
                  {item.status !== "downloading" && (
                    <button
                      className="btn btn-ghost btn-icon"
                      onClick={() => onRemoveItem(item.id)}
                      title="제거"
                    >
                      ✕
                    </button>
                  )}
                </div>
              </div>
            ))}
          </div>
          {completedCount > 0 && (
            <div className="queue-summary">
              ✓ {completedCount}개 완료
            </div>
          )}
        </section>
      )}

      {library.length > 0 && (
        <section className="card adv-card">
          <button className="adv-toggle" onClick={() => setLibOpen((v) => !v)}>
            <span className="label" style={{ margin: 0 }}>
              🗂 라이브러리
              <span className="adv-count">{library.length}</span>
            </span>
            <span className={`chevron ${libOpen ? "open" : ""}`}>▼</span>
          </button>
          {libOpen && (
            <div className="adv-body" style={{ paddingTop: 14 }}>
              <div className="queue-list">
                {library.map((item) => (
                  <div key={item.id + item.completedAt} className="queue-item">
                    {item.thumbnail && (
                      <img src={item.thumbnail} alt="" className="qthumb" />
                    )}
                    <div className="qmeta">
                      <div className="qtitle">{item.title}</div>
                      <div className="qbadges">
                        <span className="qbadge">{item.formatLabel}</span>
                        <span className="qbadge">{relativeTime(item.completedAt)}</span>
                      </div>
                    </div>
                    <div className="qactions">
                      <button
                        className="btn btn-ghost btn-icon"
                        onClick={() => onOpenLibFolder(item.outputDir)}
                        title="폴더 열기"
                      >
                        📂
                      </button>
                      <button
                        className="btn btn-ghost btn-icon"
                        onClick={() => onRemoveLibraryItem(item.id, item.completedAt)}
                        title="기록에서 제거"
                      >
                        ✕
                      </button>
                    </div>
                  </div>
                ))}
              </div>
              <div style={{ marginTop: 12, textAlign: "center" }}>
                <button className="btn btn-ghost btn-sm" onClick={onClearLibrary}>
                  라이브러리 전체 비우기
                </button>
              </div>
            </div>
          )}
        </section>
      )}

      {dragOver && (
        <div className="drop-overlay">
          <div className="drop-message">
            <div className="drop-icon">⬇</div>
            <div>동영상 링크를 여기에 놓으세요</div>
          </div>
        </div>
      )}

      {announcement && (
        <div className="modal-overlay" onClick={dismissAnnouncement}>
          <div
            className="modal-card"
            style={announcement.accent ? { borderColor: announcement.accent } : undefined}
            onClick={(e) => e.stopPropagation()}
          >
            <button
              className="modal-close"
              onClick={dismissAnnouncement}
              title="닫기 (다시 표시 안 함)"
            >
              ✕
            </button>
            {announcement.image_url && (
              <img src={announcement.image_url} className="modal-image" alt="" />
            )}
            <h2 className="modal-title">{announcement.title}</h2>
            {announcement.body && (
              <p className="modal-body">{announcement.body}</p>
            )}
            <div className="modal-actions">
              {announcement.link_url && (
                <button
                  className="btn btn-download"
                  onClick={onClickAnnouncement}
                  style={{ marginTop: 0 }}
                >
                  {announcement.button_text || "자세히 보기"}
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

export default App;
