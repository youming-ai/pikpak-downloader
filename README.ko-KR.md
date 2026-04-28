# pikpak

Rust CLI for PikPak.

## 설정

```bash
cp .env.example .env
# 수정: PIKPAK_REFRESH_TOKEN=your_token
```

Refresh Token 가져오기: [PikPak 웹](https://mypikpak.com) → DevTools → Application → Local Storage.

## 사용법

```bash
# 파일 목록
pikpak ls --path "/My Pack" -l -h

# 다운로드
pikpak download --path "/My Pack/video.mp4"

# 할당량
pikpak quota
```

## 라이선스

MIT
