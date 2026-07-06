# MK 유튜브 다운로더 (MK Downloader)

유튜브, SOOP, 트위치, 인스타, 틱톡 등 1800+ 사이트의 영상을 4K로 다운로드하는 무료 데스크톱 앱.
광고 없음, 사용자 추적 없음, 평생 무료.

- **다운로드**: https://mkvibe.com (Windows 10/11 · macOS 10.15+)
- 기술: Tauri 2 + React 19 + Rust, 엔진 yt-dlp + ffmpeg (첫 실행 시 공식 릴리스에서 자동 설치, SHA-256 검증)
- 기능: 최대 4K/MP3/GIF/자막/SponsorBlock/쇼츠 자동 생성/Premiere 편집용 ProRes 변환 — 상세는 [VERSION.md](VERSION.md)

## 빌드

```bash
npm install
npm run tauri dev    # 개발 실행
npm run tauri build  # 배포 빌드 (Windows: NSIS exe / macOS: universal dmg)
```

macOS DMG는 GitHub Actions(`.github/workflows/build.yml`)로 빌드합니다.

## 라이선스/고지

본 도구는 사용자 본인의 콘텐츠 또는 저작권자가 허락한 콘텐츠 다운로드 용도로 제공됩니다.
저작권법 준수는 사용자 책임입니다.
