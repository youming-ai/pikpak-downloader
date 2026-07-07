#[PikPak Downloader](https://github.com/youming-ai/pikpak-downloader)

[![License](https://img.shields.io/github/license/youming-ai/pikpak-downloader)](LICENSE)

Rust로 작성된 고성능 **PikPak** 클라우드 스토리지용 명령줄 도구(CLI) 및 클라이언트 라이브러리입니다.

파일 목록 확인, 계정 할당량 조회, 그리고 파일 또는 디렉터리 전체의 재귀적 다운로드를 완벽하게 지원합니다. 또한 `pikpakcli`에서 이식된 MD5 서명 생성 알고리즘을 내장하여, 로그인 시 발생하는 캡차 인증(Captcha Token)을 백그라운드에서 자동으로 해결합니다.

---

## 주요 기능

- **CLI & 라이브러리**：독립형 명령줄 도구로 직접 실행하거나, 본인의 Rust 프로젝트에 클라이언트 라이브러리로 연동하여 사용할 수 있습니다.
- **재귀 다운로드**：폴더 구조를 유지하면서 파일 또는 디렉터리 전체를 쉽게 다운로드할 수 있습니다.
- **자동 캡차 해결 및 토큰 관리**：PikPak 모바일 클라이언트의 검증 서명 알고리즘(`X-Captcha-Token`)과 토큰 로테이션을 내부적으로 자동 처리하므로 수동 캡차 풀이가 필요하지 않습니다.
- **프록시 지원**：HTTP/HTTPS 프록시 설정을 지원하여 접근이 제한된 네트워크 환경에서도 유연하게 대처할 수 있습니다.
- **상세한 파일 정보**：상세 보기(`-l`) 및 읽기 쉬운 파일 크기 단위(`-h`) 옵션이 포함된 풍부한 파일 목록 조회 기능을 제공합니다.

---

## 설치

### 소스에서 빌드

시스템에 Rust 및 Cargo가 설치되어 있는지 확인한 후 다음 명령어를 실행합니다:

```bash
# 저장소 클론
git clone https://github.com/youming-ai/pikpak-downloader.git
cd pikpak-downloader

# 릴리스 모드로 빌드
cargo build --release

# 빌드된 실행 파일은 아래 경로에서 사용할 수 있습니다:
./target/release/pikpak --help
```

### 시스템 경로에 설치

```bash
cargo install --path .
```

---

## 설정

이 프로그램은 환경 변수 또는 현재 작업 디렉터리에 있는 `.env` 파일로부터 설정을 읽어옵니다.

설정 방법:

```bash
# 환경 변수 설정 파일의 예시 복사
cp .env.example .env
```

`.env` 파일을 열고 다음과 같이 본인의 정보를 입력합니다:

```env
# 필수: PikPak 리프레시 토큰(Refresh Token)
PIKPAK_REFRESH_TOKEN=your_refresh_token_here

# 선택: HTTP/HTTPS 프록시 URL (예: http://127.0.0.1:7890)
PIKPAK_PROXY=

# 선택: 기본값을 직접 수정하려는 경우 OAuth 클라이언트 ID 및 Secret 입력
PIKPAK_CLIENT_ID=
PIKPAK_CLIENT_SECRET=
```

### `PIKPAK_REFRESH_TOKEN` 획득 방법

1. [PikPak 웹 클라이언트](https://mypikpak.com)에 접속하여 로그인합니다.
2. 브라우저의 개발자 도구를 엽니다 (보통 `F12` 키 입력 또는 우클릭 -> '검사' 선택).
3. **Application** 탭(Chrome/Edge) 또는 **저장소 (Storage)** 탭(Firefox)으로 이동합니다.
4. 왼쪽 메뉴에서 **Local Storage** -> `https://mypikpak.com`을 선택합니다.
5. `credentials`라는 이름의 키를 찾거나, 값창에서 `refresh_token`을 검색합니다. 대소문자와 숫자가 섞인 아주 긴 문자열입니다.

---

## CLI 사용법

`pikpak --help` 명령어를 실행하면 사용 가능한 모든 명령어와 플래그를 확인할 수 있습니다.

### 1. 용량(할당량) 확인

스토리지의 총 용량, 사용 중인 용량, 남은 용량을 표시합니다.

```bash
# 읽기 쉬운 포맷으로 출력 (기본값)
pikpak quota

# 출력 예시:
# total: 10.00 TiB
# used:  4.23 TiB
# free:  5.77 TiB
# usage: 42.3%

# 바이트 단위의 원본 데이터 출력
pikpak quota --raw
```

### 2. 파일 및 폴더 목록 조회

지정한 디렉터리 경로 내의 파일 목록을 조회합니다.

```bash
# 루트 폴더 (/) 아래의 모든 파일 목록 조회
pikpak ls

# 특정 경로 내의 파일 목록 조회
pikpak ls --path "/My Pack"

# 상세 포맷(-l) 및 읽기 쉬운 파일 크기 단위(-h) 적용 조회
pikpak ls --path "/My Pack" -l -h
```

### 3. 파일 및 폴더 다운로드

파일 하나 또는 디렉터리 전체를 재귀적으로 다운로드합니다.

```bash
# 기본 경로(./downloads)로 파일 다운로드
pikpak download --path "/My Pack/video.mp4"

# 디렉터리 전체를 재귀적으로 다운로드
pikpak download --path "/My Pack/Movies"

# 지정한 로컬 디렉터리로 다운로드 받기
pikpak download --path "/My Pack/video.mp4" --output "/path/to/local/dir"
```

---

## 라이브러리 사용법 (Rust API)

`pikpak`을 라이브러리 크레이트로 활용할 수도 있습니다. `Cargo.toml` 의존성에 추가하거나 로컬 경로 의존성으로 추가해 주세요.

```rust
use pikpak::{Client, FileKind};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 클라이언트 객체 생성
    let client = Client::builder()
        .refresh_token("YOUR_REFRESH_TOKEN")
        .timeout(Duration::from_secs(30))
        .proxy("http://127.0.0.1:7890") // 선택한 프록시
        .build()?;

    // 2. 스토리지 할당량 조회
    let quota = client.quota().await?;
    println!("Total quota: {} bytes, used: {} bytes", quota.total, quota.used);

    // 3. 루트 디렉터리의 파일 목록 조회
    let root_files = client.list_folder("").await?; // 루트는 빈 문자열을 전달합니다
    for file in root_files {
        println!("- {} (Kind: {:?}, Size: {} bytes)", file.name, file.kind, file.size);
    }

    // 4. 경로를 확인하고 다운로드 URL 가져오기
    let path = "/My Pack/video.mp4";
    let file_info = client.resolve_path_info(path).await?;
    if file_info.kind.is_file() {
        let download_info = client.get_download_url(&file_info.id).await?;
        println!("Download URL: {}", download_info.web_content_link);
    }

    Ok(())
}
```

---

## 라이선스

이 프로젝트는 MIT 라이선스에 따라 라이선스가 부여됩니다. 자세한 내용은 [LICENSE](LICENSE) 파일을 참조하십시오.

