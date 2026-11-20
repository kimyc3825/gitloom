# Git GUI 클라이언트 비교와 gitloom 설계 방향

> 작성 기준: 2026-11. 가격·요금제·세부 기능은 바뀔 수 있으니 각 제품 공식 사이트를 확인하세요.
> 비교는 "일상적으로 쓸 때 체감되는 수준"의 개략적인 정리입니다.

## 1. 왜 이 문서를 쓰나

Git GUI는 많지만 대부분 **한 가지씩 빠져 있습니다.**

- 커밋 그래프가 좋은 도구는 PR·이슈·CI를 보여주지 못하고,
- PR·CI를 보여주는 도구는 그래프가 없거나,
- 둘 다 있는 도구는 무겁거나, 유료 구독이거나, IDE·확장 조합에 묶여 있습니다.

이 문서는 대표 제품들의 **강점(가져올 것)** 과 **아쉬운 점(해결할 것)** 을 한곳에 모으고,
그걸 바탕으로 gitloom의 설계 원칙과 로드맵을 정합니다.

조사 대상: GitHub Desktop, Fork, GitKraken Desktop, Sourcetree, Sublime Merge, TortoiseGit,
VS Code(내장 Git + GitLens + GitHub Pull Requests and Issues), JetBrains IDE, lazygit
(+ 참고: Tower, SmartGit, GitButler, gitui)

---

## 2. 제품별 정리

### GitHub Desktop — 무료(오픈소스) · Windows/macOS

**대표 기능**
- 단순하고 친절한 변경사항/커밋 화면, 줄 단위로 골라서 커밋
- 브랜치 드롭다운의 *Pull Requests* 탭에서 PR 목록 확인·체크아웃
- 현재 브랜치 PR의 CI 체크 상태 배지, 개별/실패한 체크 재실행
- 드래그 앤 드롭으로 cherry-pick, 커밋 순서 변경·squash, amend

**아쉬운 점**
- **커밋 그래프가 없음** — 현재 브랜치의 선형 히스토리만 보임
- PR 상세(리뷰·코멘트·파일별 리뷰)와 **이슈는 브라우저로 넘어감**
- blame·파일 히스토리 없음, stash는 브랜치당 하나만 관리
- 공식 Linux 빌드 없음, GitHub 이외 호스트는 기본 Git 기능만

### Fork — 유료(1회 구매, 평가판 사용 가능) · Windows/macOS

**대표 기능**
- 빠른 네이티브 UI와 깔끔한 커밋 그래프
- 드래그 앤 드롭 인터랙티브 rebase, 머지 충돌 해결 도우미
- 줄/헝크 단위 스테이징, 이미지 diff, blame, 파일 히스토리
- stash, 서브모듈, Git LFS, Git Flow, Quick Launch(빠른 명령 실행)
- PR 생성(GitHub·GitLab·Bitbucket 등), 최근엔 AI 커밋 메시지·브랜치 리뷰 연동

**아쉬운 점**
- PR 목록·리뷰, 이슈, CI 상태를 **앱 안에서 다루기 어려움**(PR 생성 중심)
- Linux 미지원
- Windows판과 macOS판이 별도 구현이라 기능 반영 시점·UI가 다를 때가 있음

### GitKraken Desktop — 무료(로컬·공개 저장소) / private 원격 저장소는 유료 구독 · Windows/macOS/Linux

**대표 기능**
- 대표 기능인 **보기 좋은 커밋 그래프**, 드래그 앤 드롭 merge/rebase
- PR 패널: GitHub·GitLab·Bitbucket·Azure DevOps PR 조회·생성·리뷰·머지
- 이슈 트래커 연동(GitHub Issues, Jira 등), 여러 저장소의 PR·이슈를 모아보는 Launchpad
- 내장 머지 툴, 인터랙티브 rebase, AI 커밋 메시지

**아쉬운 점**
- Electron 기반이라 무겁고, 대형 저장소에서 느려지는 편
- **private 원격 저장소는 유료 구독**, 계정 로그인 필요
- 기능이 많아 화면이 복잡하고, 일부 기능은 상위 요금제 전용
- SSH·자격 증명 등 일부 설정을 앱이 따로 관리해 터미널 환경과 어긋날 수 있음

### Sourcetree — 무료(Atlassian) · Windows/macOS

**대표 기능**
- 커밋 그래프, 헝크/줄 단위 스테이징
- Git Flow 내장, 인터랙티브 rebase, stash, 서브모듈, 커스텀 액션
- Bitbucket 연동

**아쉬운 점**
- **대형 저장소에서 느리고 UI가 멈춘다**는 평이 많음
- 초기 설정에서 Atlassian 계정 연동을 유도, 개발·업데이트 속도가 느림
- GitHub PR·이슈·CI 연동 없음, 충돌 해결은 외부 머지 툴에 의존
- Windows/macOS판이 별도 구현이라 UX 차이

### Sublime Merge — 유료(평가판 사용 가능) · Windows/macOS/Linux

**대표 기능**
- 자체 UI 엔진으로 **매우 빠름**, 대형 저장소에서도 쾌적
- 3-way 머지 툴, 구문 강조 diff, 강력한 커밋 검색, blame, 파일 히스토리
- 키보드·명령 팔레트 중심
- 저장소를 바꾸는 작업은 git CLI로 실행해 **훅·설정이 터미널과 일치**

**아쉬운 점**
- **PR·이슈·CI 연동이 전혀 없음**
- 유료 라이선스

### TortoiseGit — 무료(오픈소스) · Windows 전용

**대표 기능**
- 탐색기 우클릭 메뉴와 오버레이 아이콘으로 파일 상태 표시
- 로그 창(그래프), TortoiseGitMerge(diff/3-way 머지), rebase 창, bisect
- 커밋 메시지의 이슈 번호를 이슈 트래커로 연결

**아쉬운 점**
- Windows 전용
- 기능이 **여러 대화상자로 흩어져** 저장소 전체를 한눈에 보는 메인 화면이 없음
- 오버레이 아이콘은 OneDrive 등과 슬롯을 공유해 안 보일 때가 있음
- PR·CI 연동 없음

### VS Code — 내장 Git + GitLens + GitHub Pull Requests and Issues · 무료(일부 유료) · 모든 OS

**대표 기능**
- 편집기 안에서 선택 영역 단위 스테이징, 커밋, 3-way 머지 에디터
- 내장 Source Control Graph(기본적인 그래프)
- GitLens: 줄 단위 blame, 파일·줄 히스토리, 커밋 그래프, 인터랙티브 rebase 편집기
- GitHub Pull Requests and Issues: PR 목록·체크아웃·인라인 리뷰, 이슈 목록, 이슈에서 브랜치 생성

**아쉬운 점**
- 기능이 **여러 확장에 흩어져** 있고 화면·조작 방식이 제각각
- GitLens 커밋 그래프는 로컬·공개 저장소는 무료지만 **private 저장소는 Pro(유료)**
- 편집기 레이아웃에 끼워 넣은 형태라 저장소 전체를 조망하는 화면이 좁음

### JetBrains IDE (IntelliJ, PyCharm 등) — 대부분 유료 · 모든 OS

**대표 기능**
- 그래프와 강력한 필터를 갖춘 Git 로그, 인터랙티브 rebase UI
- 최고 수준의 3-way 머지 툴, 체인지리스트·줄 단위 부분 커밋, shelve
- GitHub·GitLab PR 목록·코드 리뷰·머지

**아쉬운 점**
- IDE에 묶여 있어 무겁고, Git만 보려고 쓰기엔 과함
- 대부분 유료 라이선스, 이슈 연동은 별도 설정 필요

### lazygit — 무료(오픈소스) · 터미널 · 모든 OS

**대표 기능**
- 키보드만으로 빠른 줄/헝크 스테이징, 인터랙티브 rebase, cherry-pick, 커스텀 명령
- 가볍고, SSH로 접속한 원격 환경에서도 사용 가능

**아쉬운 점**
- 터미널 UI라 그래프·diff 시각화가 제한적, 학습 곡선이 있음
- PR·이슈·CI는 브라우저로 여는 수준

### 기타 참고

| 제품 | 특징 | 아쉬운 점 |
|---|---|---|
| Tower | Undo, PR 관리, 충돌 해결 도우미 | 유료 구독, Linux 미지원 |
| SmartGit | 그래프 + PR 연동, 모든 OS | Java 기반 UI, 상업적 사용은 유료 |
| GitButler | Rust + Tauri, 가상 브랜치(여러 브랜치 동시 작업), Undo 타임라인 | 일반적인 Git 흐름과 작업 방식이 달라 적응 필요 |
| gitui | Rust 기반의 빠른 TUI | 포지(PR·이슈) 연동 없음 |

---

## 3. 기능 비교표

✅ 지원 · △ 부분 지원/제한적 · ❌ 없음

| 기능 | GitHub Desktop | Fork | GitKraken | Sourcetree | Sublime Merge | TortoiseGit | VS Code* | JetBrains | lazygit |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| 커밋 그래프 | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | △¹ | ✅ | △ |
| 줄/헝크 스테이징 | ✅ | ✅ | ✅ | ✅ | ✅ | △ | ✅ | ✅ | ✅ |
| 인터랙티브 rebase | △ | ✅ | ✅ | ✅ | △ | ✅ | △ | ✅ | ✅ |
| 내장 충돌 해결 | △ | ✅ | ✅ | △ | ✅ | ✅ | ✅ | ✅ | △ |
| blame / 파일 히스토리 | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | △ |
| PR 목록 | ✅ | ❌² | ✅ | △³ | ❌ | ❌ | ✅ | ✅ | ❌ |
| PR 리뷰·코멘트 | ❌ | ❌ | ✅ | ❌ | ❌ | ❌ | ✅ | ✅ | ❌ |
| 이슈 | ❌ | ❌ | ✅ | ❌ | ❌ | △ | ✅ | △ | ❌ |
| CI 상태 | ✅ | ❌ | △ | ❌ | ❌ | ❌ | ✅ | △ | ❌ |
| Linux | ❌ | ❌ | ✅ | ❌ | ✅ | ❌ | ✅ | ✅ | ✅ |
| 비용 | 무료 | 유료 | 부분 유료 | 무료 | 유료 | 무료 | 무료(일부 유료) | 대부분 유료 | 무료 |

\* VS Code는 내장 Git + GitLens + GitHub Pull Requests and Issues 확장 조합 기준
¹ 내장 그래프는 기본 수준, GitLens 그래프는 private 저장소에서 유료
² PR 생성은 지원
³ Bitbucket 중심

**한 줄 요약:** 표의 위쪽(로컬 Git)과 아래쪽(협업)을 **둘 다 ✅로 채우면서 가볍고 무료인 독립 앱**이 없습니다.

---

## 4. 아쉬운 점 통합 (Pain Points)

| # | 문제 | 해당 제품 예시 |
|---|---|---|
| P1 | **로컬 히스토리와 협업 정보(PR·이슈·CI)가 분리됨.** 그래프 도구엔 PR이 없고, PR 도구엔 그래프가 없음 | Fork, Sublime Merge, Sourcetree, TortoiseGit / GitHub Desktop |
| P2 | **무겁고 느림.** Electron·IDE 기반의 메모리·시작 시간, 대형 저장소에서 UI 멈춤 | GitKraken, Sourcetree, JetBrains |
| P3 | **비용·계정 강제.** 핵심 기능(private 저장소 등)이 구독 뒤에 있거나 로그인 필요 | GitKraken, GitLens Pro, Tower |
| P4 | **플랫폼 제약.** Linux 미지원, 플랫폼별 별도 구현으로 기능 차이 | GitHub Desktop, Fork, Sourcetree, TortoiseGit |
| P5 | **터미널과 동작이 다름.** 자체 엔진·자체 설정 때문에 훅·서명·자격 증명이 CLI와 어긋날 수 있음 | 자체 Git 엔진/설정을 쓰는 클라이언트 |
| P6 | **기능 분산.** 확장 여러 개, 대화상자 여러 개로 흩어져 일관성이 떨어짐 | VS Code 확장 조합, TortoiseGit |
| P7 | **키보드 흐름 vs 시각화 양자택일.** TUI는 빠르지만 시각화가 약하고, GUI는 키보드 흐름이 약함 | lazygit, gitui / 다수 GUI |
| P8 | **실수 복구(Undo) 부재.** reset·rebase 실수를 되돌리는 기능이 일부 제품에만 있음 | 대부분 |

---

## 5. gitloom 설계 원칙

| 원칙 | 해결하는 문제 | 구현 방향 |
|---|---|---|
| **그래프 위에 협업 정보를 겹친다** | P1 | 브랜치 라벨 옆에 PR 번호와 CI 상태를 표시. PR을 고르면 그래프에서 해당 브랜치로 이동하고, 바로 체크아웃 |
| **네이티브 Rust, 단일 실행 파일** | P2, P4 | Rust + egui(즉시 모드 GUI). Windows/macOS/Linux를 같은 코드로 지원, 가상 스크롤로 큰 히스토리도 가볍게 |
| **읽기는 라이브러리, 쓰기는 진짜 git** | P2, P5 | 히스토리·diff·상태 읽기는 libgit2(`git2`)로 빠르게, 스테이징·커밋·fetch 같은 쓰기는 `git` CLI로 실행해 훅·서명·자격 증명을 터미널과 동일하게 |
| **기존 인증 재사용, 계정 강제 없음** | P3 | GitHub CLI(`gh`) 로그인을 그대로 사용. 별도 가입 없음, 오픈소스 |
| **한 화면에 다 보이게** | P6 | 브랜치 사이드바 · 히스토리/변경사항 · PR/이슈 패널을 한 창에 배치 |
| **키보드 우선** | P7 | ↑/↓ 탐색, F5 새로고침부터 시작해 명령 팔레트까지 확장 |
| **되돌릴 수 있게** | P8 | (로드맵) reflog 기반 Undo |

---

## 6. 범위와 로드맵

### v0.1 — 프로토타입 ✅
- 커밋 그래프(브랜치·태그·HEAD 라벨, 가상 스크롤, ↑/↓ 탐색)
- 커밋 상세: 메시지, 작성자, 부모, 변경 파일, diff
- 변경사항: 스테이징/언스테이징(파일 단위), 커밋(`git` CLI 사용 → 훅 실행)
- PR·이슈 패널(`gh` 사용): CI 상태, 리뷰 상태, 브라우저 열기
- **그래프 ↔ PR 연결**: 브랜치 라벨에 PR 배지, PR에서 그래프로 이동, `gh pr checkout`
- 브랜치 생성·전환, fetch/pull/push, 창에 포커스가 돌아오면 상태 자동 갱신

### v0.2 ✅ (현재)
- 헝크/줄 단위 스테이징·언스테이징·되돌리기(패치를 만들어 `git apply`로 적용)
- 파일 되돌리기·삭제, 되돌릴 수 없는 작업은 확인 창
- stash: 만들기(메시지, untracked 포함), 목록·내용 보기, apply·pop·drop
- 커밋 검색(메시지·작성자·이메일·SHA), 일치 강조와 결과 간 이동
- 로컬·원격 브랜치 삭제
- egui_kittest로 실제 UI를 조작하는 UI 테스트

### v0.3
- 드래그 앤 드롭 인터랙티브 rebase, 3-way 충돌 해결
- blame, 파일 히스토리

### v0.4 이후
- PR 상세(리뷰 코멘트, 파일별 리뷰), 이슈에서 브랜치 만들기
- GitLab 등 다른 포지 지원(포지 추상화 계층)
- 명령 팔레트, reflog 기반 Undo
- 대형 저장소: commit-graph 파일 활용·점진 로딩, gitoxide 검토
