#!/usr/bin/env bash
# Shared pinned Pat downloader; caller supplies SCRATCH and die().
PAT_VERSION=1.0.0
PAT_SHA256=331e4700c9e8b44098d7173e448da53204f59720c6c19ebc903685666aa377f7
PAT_URL="https://github.com/la5nta/pat/releases/download/v${PAT_VERSION}/pat_${PAT_VERSION}_linux_amd64.tar.gz"

fetch_pat() {
    local cache="${XDG_CACHE_HOME:-$HOME/.cache}/coppa-pat-loop/pat-$PAT_VERSION"
    local tarball="$cache/pat_${PAT_VERSION}_linux_amd64.tar.gz"
    mkdir -p "$cache"
    if [[ ! -s "$tarball" ]]; then
        command -v curl >/dev/null || die "curl is required to download Pat"
        curl -fsSL -o "$tarball" "$PAT_URL" || die "could not download $PAT_URL"
    fi
    local got
    got=$(sha256sum "$tarball" | cut -d' ' -f1)
    if [[ "$got" != "$PAT_SHA256" ]]; then
        rm -f "$tarball"
        die "Pat tarball sha256 mismatch: got $got, expected $PAT_SHA256"
    fi
    tar -xzf "$tarball" -C "$SCRATCH" 2>/dev/null
    echo "$SCRATCH/pat_${PAT_VERSION}_linux_amd64/pat"
}

