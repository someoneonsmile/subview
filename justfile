# subview — 把标准输入的字幕流实时显示到屏幕
# https://github.com/cargo/just

_default:
    @just --list

# 快速检查（类型检查，不生成二进制）
check:
    cargo check

# 编译 debug 版本
build:
    SHELL_HELP_DIR=shell_help cargo build

# 编译 release 版本（优化）
release:
    SHELL_HELP_DIR=shell_help cargo build --release

# 运行（debug）
run *args:
    cargo run -- {{args}}

# 运行（release）
run-release *args:
    cargo run --release -- {{args}}

# 格式化代码
fmt:
    cargo fmt

# 检查格式（不修改）
fmt-check:
    cargo fmt --check

# Clippy lint
lint:
    cargo clippy --all-targets

# Clippy lint（严格模式，warning 视为 error）
lint-strict:
    cargo clippy --all-targets -- -D warnings

# Clippy lint（纯终端版，严格模式）
lint-strict-terminal:
    cargo clippy --all-targets --no-default-features -- -D warnings

# Clippy 自动修复
fix:
    cargo clippy --fix --allow-dirty --allow-staged

# Clippy 自动修复 + 严格 lint 验证
fix-strict: fix lint-strict

# 运行测试
test:
    cargo test

# 运行测试（纯终端版）
test-terminal:
    cargo test --no-default-features

# 清理构建产物
clean:
    cargo clean
    rm -rf {{ SHELL_HELP_DIR }}

# ─── 安装 ──────────────────────────────────────────────

DESTDIR := ""
PREFIX := "/usr/local"
BINDIR := PREFIX / "bin"
DATADIR := PREFIX / "share"
MANDIR := DATADIR / "man"
SHELL_HELP_DIR := "shell_help"

# 安装到系统（/usr/local/bin/subview + shell 补全 + man）
install:
    SHELL_HELP_DIR={{ SHELL_HELP_DIR }} cargo build --release
    sudo install -Dm755 target/release/subview "{{ DESTDIR }}{{ BINDIR }}/subview"
    sudo install -Dm644 {{ SHELL_HELP_DIR }}/complete/subview.bash "{{ DESTDIR }}{{ DATADIR }}/bash-completion/completions/subview"
    sudo install -Dm644 {{ SHELL_HELP_DIR }}/complete/_subview "{{ DESTDIR }}{{ DATADIR }}/zsh/site-functions/_subview"
    sudo install -Dm644 {{ SHELL_HELP_DIR }}/complete/subview.fish "{{ DESTDIR }}{{ DATADIR }}/fish/vendor_completions.d/subview.fish"
    sudo install -Dm644 man/subview.1 "{{ DESTDIR }}{{ MANDIR }}/man1/subview.1"

# 从系统中卸载 subview 及其补全和 man 手册页
uninstall:
    sudo rm -f "{{ DESTDIR }}{{ BINDIR }}/subview"
    sudo rm -f "{{ DESTDIR }}{{ DATADIR }}/bash-completion/completions/subview"
    sudo rm -f "{{ DESTDIR }}{{ DATADIR }}/zsh/site-functions/_subview"
    sudo rm -f "{{ DESTDIR }}{{ DATADIR }}/fish/vendor_completions.d/subview.fish"
    sudo rm -f "{{ DESTDIR }}{{ MANDIR }}/man1/subview.1"

# 监听文件变更，自动 check
watch:
    cargo watch -x check

# 监听文件变更，自动 fmt + lint + build
watch-all:
    cargo watch -x fmt -x clippy -x build

# 全量检查（fmt → 两种特性的 clippy → 两种特性的 test → release build）
ci: fmt-check lint-strict lint-strict-terminal test test-terminal release

# ─── AUR ──────────────────────────────────────────────

# 重新生成 .SRCINFO（手动修改 PKGBUILD 后执行）
aur-srcinfo:
    cd aur && makepkg --printsrcinfo > .SRCINFO

# 提交 aur/ 变更并推送到 AUR
# AUR 禁止 force push，使用 clone → 更新文件 → commit → push 的可靠方式
aur-push:
    #!/usr/bin/env bash
    set -euo pipefail
    git add aur/
    git commit -m "chore: update AUR package" || true
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    git clone ssh://aur@aur.archlinux.org/subview-bin.git "$TMP"
    cp aur/PKGBUILD aur/.SRCINFO "$TMP/"
    git -C "$TMP" add -A
    git -C "$TMP" commit -m "chore: update AUR package" || true
    git -C "$TMP" push origin master

# 发布新版本到 AUR（自动更新版本号 + 推送）
aur-release VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    sed -i 's/^pkgver=.*/pkgver={{VERSION}}/' aur/PKGBUILD
    sed -i 's/^pkgrel=.*/pkgrel=1/' aur/PKGBUILD
    (cd aur && makepkg --printsrcinfo > .SRCINFO)
    git add aur/
    git commit -m "chore: update AUR to v{{VERSION}}" || true
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    git clone ssh://aur@aur.archlinux.org/subview-bin.git "$TMP"
    cp aur/PKGBUILD aur/.SRCINFO "$TMP/"
    git -C "$TMP" add -A
    git -C "$TMP" commit -m "chore: update AUR to v{{VERSION}}" || true
    git -C "$TMP" push origin master

# ─── Nightly AUR ───────────────────────────────────────

# 重新生成 aur-nightly/.SRCINFO
aur-nightly-srcinfo:
    cd aur-nightly && makepkg --printsrcinfo > .SRCINFO

# 提交 aur-nightly/ 变更并推送到 Nightly AUR
# AUR 禁止 force push，使用 clone → 更新文件 → commit → push 的可靠方式
aur-nightly-push:
    #!/usr/bin/env bash
    set -euo pipefail
    git add aur-nightly/
    git commit -m "chore: update nightly AUR package" || true
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    git clone ssh://aur@aur.archlinux.org/subview-nightly-bin.git "$TMP"
    cp aur-nightly/PKGBUILD aur-nightly/.SRCINFO "$TMP/"
    git -C "$TMP" add -A
    git -C "$TMP" commit -m "chore: update nightly AUR package" || true
    git -C "$TMP" push origin master

# 发布 Nightly AUR（版本号公式与 nightly workflow 一致：CARGO_VER+nightly+DATE+COMMIT_COUNT+gSHORT_SHA）
aur-nightly-release:
    #!/usr/bin/env bash
    set -euo pipefail
    DATE=$(date +%Y%m%d)
    SHORT_SHA=$(git rev-parse --short=8 HEAD)
    # 距上次稳定版 tag (v*) 的提交次数，保证与 nightly workflow 算出的版本号一致
    LAST_STABLE_TAG=$(git describe --tags --match 'v*' --abbrev=0 2>/dev/null) || true
    if [ -n "$LAST_STABLE_TAG" ]; then
      COMMIT_COUNT=$(git rev-list --count "${LAST_STABLE_TAG}..HEAD")
    else
      COMMIT_COUNT=$(git rev-list --count HEAD)
    fi
    CARGO_VER=$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)
    PKGVER="${CARGO_VER}+nightly+${DATE}+${COMMIT_COUNT}+g${SHORT_SHA}"
    sed -i "s/^pkgver=.*/pkgver=${PKGVER}/" aur-nightly/PKGBUILD
    sed -i 's/^pkgrel=.*/pkgrel=1/' aur-nightly/PKGBUILD
    (cd aur-nightly && makepkg --printsrcinfo > .SRCINFO)
    git add aur-nightly/
    git commit -m "chore: update nightly AUR to ${PKGVER}" || true
    TMP=$(mktemp -d)
    trap 'rm -rf "$TMP"' EXIT
    git clone ssh://aur@aur.archlinux.org/subview-nightly-bin.git "$TMP"
    cp aur-nightly/PKGBUILD aur-nightly/.SRCINFO "$TMP/"
    git -C "$TMP" add -A
    git -C "$TMP" commit -m "chore: update nightly AUR to ${PKGVER}" || true
    git -C "$TMP" push origin master
