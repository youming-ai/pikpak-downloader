# PikPak 개인 클라우드 관리 도구 v4.0 🚀

Go 언어로 작성된 PikPak 개인 클라우드 관리 도구입니다. PikPak 클라우드 파일을 관리하기 위한 완전한 명령줄 인터페이스를 제공합니다.

## ✨ 핵심 기능

### 🎯 개인 클라우드 관리
- **파일 목록** - 개인 클라우드의 파일과 폴더를 탐색하고 확인
- **할당량 보기** - 실시간 클라우드 저장 공간 사용량 확인
- **파일 다운로드** - 개인 클라우드의 모든 파일 또는 전체 폴더 다운로드
- **스마트 분류** - 자동 파일 유형 인식 (비디오, 이미지, 문서 등)

### 🔧 기술적 우위
- **Go 언어 개발** - 고성능, 낮은 리소스 사용량
- **명령줄 인터페이스** - 간단하고 사용하기 쉬운 CLI 도구
- **환경 변수 설정** - 보안된 설정 관리 솔루션
- **동시 다운로드** - 멀티스레드 동시 다운로드 지원
- **진행률 표시** - 실시간 다운로드 진행률 표시

## 🚀 빠른 시작

### 1. 의존성 설치
```bash
make deps
```

### 2. 인증 정보 설정

설정 파일 생성:
```bash
cp .env.example .env
```

`.env` 파일을 편집하고 PikPak 인증 정보를 입력:
```bash
# 방법 1: 계정과 비밀번호 사용
PIKPAK_USERNAME=[당신의_이메일_주소]
PIKPAK_PASSWORD=[당신의_비밀번호]

# 방법 2: RefreshToken 사용 (권장)
PIKPAK_REFRESH_TOKEN=[당신의_refresh_token]
```

### 3. 프로그램 컴파일
```bash
make build-cli
```

### 4. 사용 시작
```bash
# 도움말 보기
./pikpak-cli help

# 클라우드 할당량 보기
./pikpak-cli quota

# 루트 디렉토리 파일 목록
./pikpak-cli ls

# 지정된 디렉토리 목록
./pikpak-cli ls -path "/My Pack"

# 상세 목록
./pikpak-cli ls -path "/My Pack" -l -h

# 파일 다운로드
./pikpak-cli download -path "/My Pack/document.pdf"

# 전체 폴더 다운로드
./pikpak-cli download -path "/My Pack" -output "./downloads"
```

## 📋 명령어 상세

### `ls` - 파일 목록
```bash
./pikpak-cli ls [옵션]

옵션:
  -path string     디렉토리 경로 (기본값: "/")
  -l               긴 형식 표시
  -h               사람이 읽기 쉬운 형식

예시:
  ./pikpak-cli ls                          # 루트 디렉토리 목록
  ./pikpak-cli ls -path "/My Pack"        # 지정된 디렉토리 목록
  ./pikpak-cli ls -l -h                     # 상세 형식
```

### `quota` - 할당량 보기
```bash
./pikpak-cli quota [옵션]

옵션:
  -h               사람이 읽기 쉬운 형식 (기본값: true)

예시:
  ./pikpak-cli quota                       # 할당량 정보 보기
```

### `download` - 파일 다운로드
```bash
./pikpak-cli download [옵션]

옵션:
  -path string     다운로드 경로 (기본값: "/")
  -output string   출력 디렉토리 (기본값: "./downloads")
  -count int       동시 실행 수 (기본값: 3)
  -progress        진행률 표시 (기본값: true)

예시:
  ./pikpak-cli download -path "/My Pack/video.mp4"                    # 단일 파일 다운로드
  ./pikpak-cli download -path "/My Pack" -output "./my_downloads"   # 지정된 디렉토리에 다운로드
  ./pikpak-cli download -path "/My Pack" -count 5                    # 동시 실행 수 설정
```

## 🛠️ 개발 및 빌드

### 컴파일
```bash
make build-cli
```

### 실행
```bash
# Makefile 사용
make run-cli ls
make run-cli quota
make run-cli download -path "/My Pack"

# 직접 실행
./pikpak-cli ls
```

### 정리
```bash
make clean
```

## 📊 기능 데모

### 할당량 정보 보기
```bash
$ ./pikpak-cli quota
📊 클라우드 할당량 정보:
전체: 6.0GB
사용됨: 604.2MB
사용률: 9.8%
```

### 파일 목록
```bash
$ ./pikpak-cli ls
폴더        My Pack
폴더        Pack From Shared

$ ./pikpak-cli ls -path "/Pack From Shared"
폴더        onlyfans chaeira 34V
```

### 상세 목록
```bash
$ ./pikpak-cli ls -l -h
타입        크기       수정 시간           이름
폴더        -          2025-01-02 15:04   My Pack
폴더        -          2025-01-01 10:30   Pack From Shared
```

## 📁 프로젝트 구조

```
pikpak-downloader/
├── pikpak_cli.go           # CLI 명령줄 인터페이스
├── pikpak_client.go        # PikPak 클라이언트 핵심 기능
├── config_manager.go       # 설정 관리
├── .env                     # 사용자 설정 파일
├── .env.example            # 설정 파일 템플릿
├── pikpak-cli              # 실행 파일
├── Makefile                 # 빌드 스크립트
├── go.mod                   # Go 모듈 파일
├── go.sum                   # 의존성 검증 파일
├── README.ko-KR.md          # 한국어 프로젝트 설명
└── .gitignore               # Git 무시 파일
```

## ⚙️ 설정 설명

### 환경 변수 설정
`.env` 파일에 다음 정보를 설정:

```bash
# PikPak 계정 인증
PIKPAK_USERNAME=[당신의_이메일_주소]
PIKPAK_PASSWORD=[당신의_비밀번호]

# 또는 RefreshToken 사용 (권장)
PIKPAK_REFRESH_TOKEN=[당신의_refresh_token]

# 프록시 설정 (선택 사항)
# PIKPAK_PROXY=http://127.0.0.1:7890

# 장치 설정 (선택 사항)
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### RefreshToken 가져오기
1. PikPak 웹 버전에 로그인
2. `F12` 키로 개발자 도구 열기
3. `Application` → `Local Storage`로 이동
4. `refresh_token` 필드를 찾아 값 복사
5. `.env` 파일의 `PIKPAK_REFRESH_TOKEN`에 입력

## 🔄 버전 기록

### v4.0.0 (2025-10-18) 🎯
- ✨ **프로젝트 위치 재설계** - 개인 클라우드 관리에 집중
- 🎯 **pikpakcli 기능 복제** - 파일 목록, 할당량 보기, 파일 다운로드
- 🔧 **완전한 CLI 인터페이스** - 매개변수 파싱, 도움말 시스템
- 📋 **스마트 파일 분류** - 자동 파일 유형 인식
- ⚙️ **설정 관리 최적화** - 환경 변수 설정 솔루션

### v3.1.0 (2025-10-18) 🌟
- ✨ **.env 설정 지원** - 환경 변수 설정 솔루션 추가, 더 안전하고 편리
- 🔄 **자동 설정 생성** - 프로그램이 .env를 자동으로 읽고 pikpakcli 설정 파일 생성
- 📋 **설정 상태 확인** - 상세한 설정 검증 및 상태 표시
- 🔧 **설정 관리자** - config_manager.go 모듈 추가
- 🎯 **기본 CLI 모드** - 하이브리드 도구가 기본적으로 CLI 모드 사용, 3가지 모드 선택 지원
- 📁 **설정 파일 템플릿** - .env.example 템플릿 파일 제공

### v3.x.x (공유 링크 다운로드)
- 하이브리드 다운로드 모드
- 웹 크롤러 기능
- 공유 링크 처리

### v2.x.x (공유 링크 다운로드)
- Go 버전 재작성
- 기본 다운로드 기능

### v1.x.x (Python 버전)
- 초기 구현

## 🤝 기여

Issue와 Pull Request를 환영합니다!

1. 이 프로젝트 Fork
2. 기능 브랜치 생성 (`git checkout -b feature/AmazingFeature`)
3. 변경사항 커밋 (`git commit -m 'Add some AmazingFeature'`)
4. 브랜치에 푸시 (`git push origin feature/AmazingFeature`)
5. Pull Request 열기

## 📄 라이선스

이 프로젝트는 MIT 라이선스를 따릅니다 - 자세한 내용은 [LICENSE](LICENSE) 파일을 확인하세요.

## ⚠️ 면책조항

이 도구는 개인 클라우드 관리 전용입니다. PikPak의 서비스 약관을 준수하고, 상업적 목적이나 저작권법을 위반하는 콘텐츠에 사용하지 마세요. 개발자는 법적 책임을 지지 않습니다.

## 🙏 감사의 말

- [pikpakcli](https://github.com/52funny/pikpakcli) - 핵심 기능 참조
- Go 언어 커뮤니티 - 우수한 개발 도구와 라이브러리

---

이 프로젝트가 도움이 되셨다면 ⭐️를 남겨주세요!