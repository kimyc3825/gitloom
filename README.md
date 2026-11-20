# gitloom

**커밋 그래프 · 변경사항 · Pull Request · 이슈 · CI를 한 화면에 엮는 가벼운 네이티브 Git GUI**

Rust와 [egui](https://github.com/emilk/egui)로 만든 크로스 플랫폼 데스크톱 앱입니다.
그래프가 좋은 Git 클라이언트는 PR·CI를 보여주지 못하고, PR을 보여주는 클라이언트는 그래프가 없는 문제를 풀려고 시작했습니다.
배경과 제품 비교는 [docs/research.md](docs/research.md)에 정리했습니다.

> 초기 프로토타입(v0.1)입니다. 중요한 저장소에서는 주의해서 사용하세요.

![gitloom 스크린샷](docs/screenshot.png)

## 주요 기능 (v0.1)

- **커밋 그래프**: 브랜치·태그·HEAD 라벨, 레인 색상, 머지 표시, 가상 스크롤(최근 커밋 20,000개), 키보드 탐색
- **커밋 상세**: 메시지·작성자·부모 커밋, 변경 파일 목록, 줄 번호가 있는 diff
- **변경사항**: 파일 단위 스테이징/언스테이징, 커밋과 amend(Ctrl+Enter)
- **GitHub 연동**: 열린 PR(CI·리뷰 상태)과 이슈, PR별 체크 목록, 브라우저로 열기
- **그래프 ↔ PR 연결**: 브랜치 라벨 옆에 PR 번호와 CI 상태를 표시하고, PR에서 그래프로 이동하거나 `gh pr checkout`으로 바로 체크아웃
- **브랜치 작업**: 전환, 원격 브랜치 체크아웃, 브랜치 생성, detached 체크아웃, fetch/pull/push
- 한글 등 CJK 커밋 메시지 표시(시스템 폰트 자동 사용), 저장소 폴더 드래그 앤 드롭

## 설계 원칙

| 원칙 | 내용 |
|---|---|
| 읽기는 빠르게 | 히스토리·diff·상태는 libgit2(`git2`)로 직접 읽고, 로딩은 모두 백그라운드 스레드에서 처리 |
| 쓰기는 진짜 `git`으로 | 스테이징·커밋·fetch 등은 `git` CLI를 실행해서 훅, 커밋 서명, 자격 증명, 설정이 터미널과 똑같이 동작 |
| 인증은 재사용 | GitHub 데이터는 GitHub CLI(`gh`)로 가져와서 별도 로그인이나 토큰 저장이 없음 |
| 네이티브 단일 바이너리 | Electron 없이 Rust + egui로 Windows/macOS/Linux 지원 |

## 시작하기

### 필요 조건

- Rust 1.88 이상 ([rustup](https://rustup.rs))
- `git` (PATH에 있어야 함)
- (선택) [GitHub CLI](https://cli.github.com): `gh auth login`을 해 두면 PR·이슈 패널을 쓸 수 있음
- Windows는 Visual Studio C++ Build Tools, Linux는 eframe·rfd가 요구하는 시스템 라이브러리

### 실행

```bash
git clone https://github.com/kimyc3825/gitloom
cd gitloom
cargo run --release -- /path/to/your/repo
```

경로를 생략하면 현재 디렉터리가 저장소일 때 그 저장소를 열고, 아니면 시작 화면에서 폴더를 고를 수 있습니다.

### 조작

| 동작 | 방법 |
|---|---|
| 커밋 이동 | ↑ / ↓ / PgUp / PgDn / Home / End |
| 새로고침 | F5 (창에 포커스가 돌아오면 자동) |
| 커밋 | 메시지 입력 후 Ctrl+Enter |
| 브랜치 전환 | 사이드바에서 브랜치 더블클릭 |
| 커밋 메뉴 | 그래프에서 우클릭 (SHA 복사, 브랜치 생성, 체크아웃) |
| 스테이징 | 파일 옆 `+` / `−` 버튼 또는 더블클릭 |
| PR 브랜치로 이동 | PR 카드 더블클릭 또는 "Show in graph" |

## 구조

```
src/
├── main.rs             진입점, 창 설정
├── app/                앱 상태와 화면
│   ├── mod.rs          상태, 백그라운드 결과 처리, 툴바·상태바
│   ├── history.rs      커밋 그래프 + 커밋 상세
│   ├── changes.rs      스테이징·커밋
│   ├── sidebar.rs      브랜치·원격·태그
│   └── forge_panel.rs  PR·이슈·CI 패널
├── git/
│   ├── graph.rs        커밋 그래프 레인 배치 알고리즘
│   ├── repo.rs         libgit2 읽기: 히스토리, ref, diff, 상태
│   └── ops.rs          git CLI 쓰기: stage, commit, fetch, switch …
├── forge/
│   ├── mod.rs          PR·이슈·체크 모델
│   └── github.rs       gh CLI 연동 (JSON 파싱)
├── ui/                 그래프 행, diff 뷰, 라벨(chip) 그리기
├── jobs.rs             백그라운드 작업 (스레드 + 채널)
├── fonts.rs            CJK 폰트 폴백
└── util.rs             시간 포맷, 프로세스 실행
```

## 개발

```bash
cargo test                   # 그래프 레이아웃, gh JSON 파싱, git 연동 테스트
cargo clippy --all-targets
cargo fmt
```

## 로드맵

헝크/줄 단위 스테이징, stash, 검색 → 인터랙티브 rebase, 3-way 충돌 해결, blame →
PR 리뷰 코멘트, GitLab 지원, 명령 팔레트, reflog 기반 Undo.
자세한 내용은 [docs/research.md의 로드맵](docs/research.md#6-범위와-로드맵)을 참고하세요.

## 라이선스

[MIT](LICENSE-MIT) 또는 [Apache-2.0](LICENSE-APACHE) 중 원하는 쪽을 선택해 사용할 수 있습니다.
