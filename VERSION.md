# MK 유튜브 다운로더 — Basic 모델

## v1.0.2 macOS 지원 추가 (2026-07-07)

- **macOS 포팅**: Intel/Apple Silicon universal DMG (macOS 10.15+, 무서명 — 첫 실행 시 우클릭→열기)
- yt-dlp는 공식 `yt-dlp_macos`, ffmpeg/ffprobe는 검증본 미러(GitHub Releases, zip+실행파일 이중 SHA-256)
- 맥 앱 업데이트는 DMG 다운로드+검증 후 열어주는 방식 (수동 교체)
- version.json `platforms` 스키마 도입 (Windows 하위호환 유지)
- GitHub Actions CI 빌드 (`5megakim/mkdownloader`)

## v1.0.2 (2026-07-06) 마이너 업데이트

1. **✂️ Premiere 편집용 형식 추가**: 최고화질(AV1 포함) 그대로 받은 뒤 프리미어 프로가 지원하는 MOV(ProRes 422 HQ + PCM 48kHz)로 자동 변환. 화질 보존·용량 큼. 자막은 임베드 대신 외부 SRT 저장.
2. **시작메뉴/윈도우 검색 수정**: productName을 "MK 유튜브 다운로더"로 변경 (기존 "youtubedownloader"라 한글 검색 안 잡힘), exe명은 mainBinaryName=MKDownloader로 고정, 시작메뉴 폴더 "MK Vibe". identifier는 유지(com.megakim.youtubedownloader — 앱데이터/업그레이드 연속성).
3. **앱 배포 업데이트 알림**: mkvibe.com/megabite/version.json을 확인해 새 배포판이 있으면 업데이트 배너 표시(엔진 업데이트보다 우선) → 다운로드+SHA256 검증+설치기 실행.

**버전 동결 시점**: 2026-05-12 (v1.0.1까지)

## 포함 기능

### 1단계: 핵심 다운로드
- 유튜브 URL 자동 클립보드 감지 + 창 포커스 시 자동 붙여넣기
- 영상 정보 실시간 미리보기 (썸네일/제목/업로더/조회수/길이)
- 최대 화질·음질 뱃지 표시 (코덱 포함)
- MP4/MP3/WebM 형식 선택
- 4K/1440p/1080p/720p/480p 화질 선택
- 저장 폴더 지정 (localStorage 기억) + 폴더 열기 버튼
- 다운로드 진행률 (속도/남은시간/총 크기)

### 2단계: 모던 UX
- 드래그앤드롭 URL
- 다운로드 큐 (순차 처리)
- 시스템 트레이 상주 (창 닫아도 백그라운드)
- Windows 토스트 알림
- yt-dlp 자동 업데이트 체크 + SHA-256 체크섬 검증

### 3단계: 파워 유저
- 자막 다운로드 (다국어 SRT + 자동생성 + 임베드)
- SponsorBlock 광고/스폰서 자동 제거
- 챕터별 분할
- 다운로드 속도 제한
- 브라우저 쿠키 (멤버십/연령제한 영상)

### 4단계: 기본 AI/특수 기능
- 구간 자르기 (시작-끝 시간 지정)
- GIF 움짤 변환 (FPS/너비 조정)
- 라이브 스트림 감지 + 처음부터 녹화 옵션
- 라이브러리 히스토리 (최근 30개)
- **쇼츠 자동 생성** (heatmap 기반)
  - 1~10개 자동 추출
  - 15/30/45/60초 길이
  - 9:16 가운데 크롭 또는 블러 패딩
  - 1080×1920 Full HD 세로 출력

## 기술 스택
- **프론트엔드**: Tauri 2 + React 19 + TypeScript + Vite 7
- **백엔드**: Rust (tokio, reqwest, sha2)
- **번들 바이너리**: yt-dlp.exe + ffmpeg.exe + ffprobe.exe
- **플러그인**: dialog, clipboard-manager, opener, notification, tray-icon

## 의도적 제외 (Standard 모델에서 추가 예정)
- Whisper 로컬 자막 생성
- Claude API 기반 AI 분석
- 맥락 인식 하이라이트 교차검증
- 자동 자막 임베드 + 1.2배속 처리
- GPU 가속 인코딩
