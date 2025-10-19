# PikPak 개인 클라우드 관리 도구 v4.0 🚀

PikPak 개인 클라우드 스토리지를 관리하는 고성능 Go CLI 도구입니다.

## ✨ 기능

- **📁 파일 관리** - 클라우드 파일 목록, 검색, 정리
- **💾 파일 다운로드** - 개별 파일 또는 전체 폴더 다운로드
- **📊 스토리지 모니터** - 실시간 할당량 및 사용량 정보
- **⚡ 고성능** - 진행률 표시와 동시 다운로드
- **🔒 보안 설정** - 환경 변수 기반 인증

## 🚀 빠른 시작

### 1. 의존성 설치
```bash
make deps
```

### 2. 인증 정보 설정
```bash
cp .env.example .env
```

`.env` 파일 편집:
```bash
# 방법 1: 계정과 비밀번호
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]

# 방법 2: RefreshToken (권장)
PIKPAK_REFRESH_TOKEN=[your_refresh_token]
```

### 3. 빌드 및 실행
```bash
make build-cli
./pikpak-cli help
```

## 📋 명령어

### 파일 목록
```bash
./pikpak-cli ls                    # 루트 디렉토리
./pikpak-cli ls -path "/My Pack"   # 특정 폴더
./pikpak-cli ls -l -h              # 상세 보기
```

### 스토리지 할당량
```bash
./pikpak-cli quota                 # 스토리지 사용량 보기
```

### 파일 다운로드
```bash
./pikpak-cli download -path "/My Pack/file.pdf"                    # 단일 파일
./pikpak-cli download -path "/My Pack" -output "./downloads"      # 전체 폴더
./pikpak-cli download -path "/My Pack" -count 5                   # 동시 실행 수 설정
```

## 📁 프로젝트 구조

```
pikpak-downloader/
├── pikpak_cli.go           # CLI 인터페이스
├── pikpak_client.go        # 핵심 클라이언트 기능
├── config_manager.go       # 설정 관리
├── .env.example            # 설정 템플릿
├── Makefile                # 빌드 자동화
└── README*.md              # 문서
```

## ⚙️ 설정

### 환경 변수 (.env)
```bash
# 인증
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]
# 또는
PIKPAK_REFRESH_TOKEN=[your_refresh_token]

# 선택 사항
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### RefreshToken 가져오기
1. PikPak 웹 버전에 로그인
2. 개발자 도구 열기 (F12)
3. `Application` → `Local Storage` 이동
4. `refresh_token` 값 복사
5. `.env` 파일에 추가

## 🔄 버전 기록

### v4.0.0 (2025-10-18) 🎯
- **개인 클라우드 관리** - 파일 관리에 특화된 완전한 재작성
- **CLI 인터페이스** - 도움말 시스템 포함 완전한 명령줄 인터페이스
- **스마트 파일 분류** - 자동 파일 유형 인식
- **환경 변수 설정** - 보안 .env 기반 설정

### v3.1.0 (2025-10-18) 🌟
- .env 설정 지원 추가
- 자동 설정 생성
- 보안 및 사용성 향상

## 🛠️ 개발

```bash
make build-cli    # CLI 도구 빌드
make clean        # 빌드 산물 정리
make run-cli ls   # 예제 명령어로 실행
```

## 🤝 기여

1. 프로젝트 Fork
2. 기능 브랜치 생성 (`git checkout -b feature/AmazingFeature`)
3. 변경사항 커밋 (`git commit -m 'Add AmazingFeature'`)
4. 브랜치에 푸시 (`git push origin feature/AmazingFeature`)
5. Pull Request 열기

## 📄 라이선스

MIT 라이선스 - 자세한 내용은 [LICENSE](LICENSE) 파일을 확인하세요.

## ⚠️ 면책조항

이 도구는 개인 클라우드 관리 전용입니다. PikPak 서비스 약관과 저작권법을 준수해 주세요. 개발자는 법적 책임을 지지 않습니다.

## 🙏 감사의 말

- [pikpakcli](https://github.com/52funny/pikpakcli) - 핵심 기능 참조
- Go 언어 커뮤니티 - 훌륭한 개발 도구와 라이브러리

---

이 프로젝트가 도움이 되셨다면 ⭐️를 남겨주세요!