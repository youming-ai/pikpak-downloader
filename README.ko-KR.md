# PikPak 개인 클라우드 관리 도구

**PikPak API를 직접 호출하는 네이티브 Rust CLI 도구.**

> 외부 `pikpakcli` 바이너리가 필요 없습니다 — 모든 기능은 네이티브 API 호출로 구현됩니다.

## 🌍 언어

- 🇺🇸 **[English](./README.md)** | 🇨🇳 **[中文](./README.zh-CN.md)** | 🇯🇵 **[日本語](./README.ja-JP.md)** | 🇰🇷 **한국어**

## ✨ 기능

### 📁 파일 관리
- **`ls --path "/My Pack"`** — 경로 기반 탐색 (내부에서 folder id 자동 해석)
- **긴 형식** (`-l`)로 종류와 크기 표시
- **사람이 읽기 쉬운 크기** (`-h`): KB / MB / GB

### ⬇️ 다운로드
- **네이티브 다운로드** — API의 직접 다운로드 URL을 스트리밍
- **단일 파일** 또는 **재귀 폴더** 다운로드
- **진행률 표시**에 퍼센트

### 📊 계정 정보
- **`quota`** — 저장소 사용량 (사람이 읽기 쉬운 형식 또는 원시 바이트)

### ⚙️ 빌드
- **단일 ~3.9 MB 바이너리** (정적 링크, 런타임 의존성 없음)
- **크로스 플랫폼** Rust (Linux, macOS, Windows)

## 🚀 빠른 시작

### 전제 조건
- **Rust 1.78+** (소스에서 빌드)
- **PikPak 계정** 및 refresh token

### 소스에서 설치

```bash
git clone https://github.com/your-username/pikpak-downloader.git
cd pikpak-downloader
make build
```

바이너리는 `rust/target/release/pikpak-cli`에 생성됩니다.

### 인증 설정

```bash
cp .env.example .env
# .env 편집:
PIKPAK_REFRESH_TOKEN=your_refresh_token
# 선택사항: PIKPAK_PROXY=http://127.0.0.1:7890
```

### 사용법

```bash
# 루트 디렉터리 목록
make run ARGS='ls'

# 상세보기 + 사람이 읽기 쉬운 크기
make run ARGS='ls --path "/My Pack" -l -h'

# 파일 다운로드
make run ARGS='download --path "/My Pack/video.mp4" --output ./downloads'

# 할당량 확인
make run ARGS='quota'
```

또는 바이너리를 직접 실행:

```bash
./rust/target/release/pikpak-cli help
```

## 📋 CLI 참조

### `ls` — 파일 및 디렉터리 목록

| 옵션 | 설명 |
|------|------|
| `--path <path>` | 디렉터리 경로, 예: `"/My Pack"` (기본값: `/`) |
| `-l, --long` | 긴 형식 (종류 및 크기) |
| `-h, --human` | 사람이 읽기 쉬운 크기 (KB / MB / GB) |

```bash
pikpak-cli ls --path "/My Pack" -l -h
```

### `download` — 파일 또는 폴더 다운로드

| 옵션 | 설명 |
|------|------|
| `--path <path>` | 다운로드할 원격 경로 (필수) |
| `--output <dir>` | 로컬 출력 디렉터리 (기본값: `./downloads`) |
| `--count <n>` | 동시성 힌트 (기본값: `3`) |

```bash
# 단일 파일
pikpak-cli download --path "/My Pack/document.pdf" --output ./downloads

# 전체 폴더 (재귀)
pikpak-cli download --path "/My Photos" --output ./backups
```

### `quota` — 저장소 할당량 보기

| 옵션 | 설명 |
|------|------|
| `--raw` | 사람이 읽기 쉬운 형식 대신 원시 바이트 수 출력 |

```bash
pikpak-cli quota
pikpak-cli quota --raw
```

## 🏗️ 작동 방식

1. **인증** — refresh token으로 단기 access token을 교환합니다. 토큰은 매번 로테이션되며, `TokenManager::current_refresh_token`으로 가져와 영속화할 수 있습니다.
2. **캡차** — 각 drive API 호출에 대해 `X-Captcha-Token`을 요청합니다. 만료 시 자동 갱신됩니다.
3. **경로 해석** — `list_folder`를 통해 각 세그먼트를 거쳐 `"/My Pack/videos"` → `folder_id`를 해석합니다.
4. **다운로드** — API에서 `web_content_link`를 가져온 다음, 바이트를 스트리밍하여 디스크에 기록하고 진행률 바를 표시합니다.

## 📁 프로젝트 구조

```
pikpak-downloader/
├── rust/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── pikpak-api/          # 라이브러리 crate (인증, 캡차, API 클라이언트)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs        # OAuth2 리프레시 토큰 흐름
│   │       ├── captcha.rs     # MD5 체인 서명 계산
│   │       ├── client.rs      # API: quota, list_folder, resolve_path, download
│   │       ├── error.rs       # 오류 열거형
│   │       └── types.rs       # FileInfo, FileKind, Quota
│   └── pikpak-cli/            # 바이너리 crate (CLI 프론트엔드)
│       └── src/main.rs        # Clap CLI + tokio 비동기 런타임
├── .env.example
├── Makefile
├── README.md
└── ...
```

## ⚙️ 구성

### 환경 변수 (`.env`)

```bash
# 필수
PIKPAK_REFRESH_TOKEN=your_refresh_token

# 선택사항
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader

# OAuth 재정의 (설정하지 않으면 기본값 사용)
# PIKPAK_CLIENT_ID=
# PIKPAK_CLIENT_SECRET=
```

### Refresh Token 가져오기

1. [PikPak 웹 버전](https://mypikpak.com)에 로그인
2. 개발자 도구 열기 (F12)
3. **Application → Local Storage → `https://mypikpak.com`** 이동
4. `refresh_token` 값 복사
5. `.env`에 붙여넣기: `PIKPAK_REFRESH_TOKEN=...`

## 🔄 최근 변경 사항

| 커밋 | 변경 |
|------|------|
| `38f65ac` | **Rust 완전 재작성** — Go wrapper 제거 (외부 `pikpakcli` 바이너리 불필요) |
| — | 네이티브 다운로드 및 경로 해석 추가 |
| — | PrintStats 영 나누기 분할 panic, quoteString YAML 이스케이프 버그, PerformanceMetrics 혼합 동기화, detectFileType 중복 할당, ListFilesStream stdout 닫기 문제, Rust 토큰 갱신 폭풍, captcha double-check 레이스 수정 |

## 📄 라이선스

MIT — 자세한 내용은 [LICENSE](LICENSE)를 참조하십시오.

## 🙏 감사의 말

- 엔드포인트 및 캡차 로직은 [`pikpakcli`](https://github.com/52funny/pikpakcli) (Go)에서 리버스 엔지니어링하여 네이티브 Rust로 이식되었습니다.
- PikPak은 공식 API를 게시하지 않으므로 엔드포인트는 통보 없이 변경될 수 있습니다.
