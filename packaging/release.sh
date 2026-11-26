#!/usr/bin/env bash
set -euo pipefail

asset=Tenka-x86_64.AppImage

fetch() {
    git fetch origin --tags '+refs/heads/*:refs/remotes/origin/*'
}

prepare() {
    local channel=$1 current major minor patch version tag release_commit candidate
    fetch
    if [[ "$channel" == stable ]]; then
        local parents
        read -ra parents <<< "$(git rev-list --parents -n 1 "$GITHUB_SHA")"
        if [[ ${#parents[@]} != 3 ]] || ! git merge-base --is-ancestor "${parents[2]}" origin/dev; then
            echo 'Only a dev merge creates a stable release.'
            return
        fi
        if git rev-parse --verify --quiet origin/deps >/dev/null &&
            [[ $(git rev-list --count origin/dev..origin/deps) != 0 ]]; then
            echo 'Merge and test the pending deps changes in dev before publishing.' >&2
            return 1
        fi
    elif [[ -z $(git log --format=%H --no-merges origin/main..HEAD) ]]; then
        echo 'No dependency changes to publish.'
        return
    elif ! git merge-base --is-ancestor origin/main HEAD; then
        echo 'Sync deps with main before publishing.' >&2
        return 1
    fi

    current=$(git show "$GITHUB_SHA:VERSION")
    [[ "$current" =~ ^[0-9]+\.[0-9]+\.0$ ]]
    IFS=. read -r major minor patch <<< "$current"
    if [[ "$channel" == stable ]]; then
        version="$major.$((minor + 1)).0"
    else
        while IFS= read -r candidate; do
            [[ "$candidate" =~ ^v[0-9]+\.[0-9]+\.[1-9][0-9]*$ ]] || continue
            [[ $(git rev-parse "refs/tags/$candidate^{commit}^") == "$GITHUB_SHA" ]] || continue
            if [[ $(gh release view "$candidate" --json isDraft --jq .isDraft 2>/dev/null || true) == false ]]; then
                echo 'This dependency commit already has a verified prerelease.'
                return
            fi
            version=${candidate#v}
            break
        done < <(git tag --list "v$major.$minor.*" --sort=-version:refname)
        patch=$(git tag --list "v$major.$minor.*" | awk -F. '$3 ~ /^[0-9]+$/ && $3 > n {n=$3} END {print n+1}')
        version=${version:-"$major.$minor.$patch"}
    fi
    tag="v$version"
    git checkout --detach "$GITHUB_SHA"
    python3 packaging/set-version.py "$current" "$version"
    if git rev-parse --verify --quiet "refs/tags/$tag^{commit}" >/dev/null; then
        release_commit=$(git rev-parse "refs/tags/$tag^{commit}")
        [[ $(git rev-parse "$release_commit^") == "$GITHUB_SHA" ]]
        git diff --exit-code "$release_commit" -- VERSION Cargo.toml Cargo.lock
    fi
    printf 'version=%s\n' "$version" >> "$GITHUB_OUTPUT"
}

publish() {
    local channel=$1 version=$2 tag="v$2" release_commit state verification
    local files=("$asset" SHA256SUMS)
    local flags=(--prerelease) draft_flags=(--prerelease)
    if [[ "$channel" == stable ]]; then
        files+=("$asset.zsync")
        flags=(--latest)
        draft_flags=()
    fi
    (cd dist && sha256sum --check SHA256SUMS)
    fetch
    if release_commit=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}"); then
        [[ $(git rev-parse "$release_commit^") == "$GITHUB_SHA" ]]
        git diff --exit-code "$release_commit" -- VERSION Cargo.toml Cargo.lock
    else
        [[ $(git rev-parse "origin/${GITHUB_REF_NAME}") == "$GITHUB_SHA" ]]
        git add VERSION Cargo.toml Cargo.lock
        git -c user.name='github-actions[bot]' -c user.email='41898282+github-actions[bot]@users.noreply.github.com' \
            commit -m "Release $tag"
        release_commit=$(git rev-parse HEAD)
        git tag "$tag" "$release_commit"
        if [[ "$channel" == stable ]]; then
            git push --atomic origin "$release_commit:refs/heads/main" "refs/tags/$tag"
        else
            git push origin "refs/tags/$tag"
        fi
    fi

    verification=$(mktemp -d)
    trap 'rm -rf "$verification"' RETURN
    (
        cd dist
        if ! gh release view "$tag" --json isDraft > "$verification/state.json" 2>/dev/null; then
            gh release create "$tag" "${files[@]}" --target "$release_commit" --verify-tag \
                --title "Tenka $tag" --generate-notes --draft "${draft_flags[@]}"
            state=true
        else
            state=$(jq -r .isDraft "$verification/state.json")
            if [[ "$state" == true ]]; then
                gh release upload "$tag" "${files[@]}" --clobber
            fi
        fi
        gh release download "$tag" --dir "$verification" --pattern 'Tenka-x86_64.AppImage*' --pattern SHA256SUMS
        cmp SHA256SUMS "$verification/SHA256SUMS"
        (cd "$verification" && sha256sum --check SHA256SUMS)
        if [[ "$state" == true ]]; then
            gh release edit "$tag" --draft=false "${flags[@]}"
        fi
        gh release view "$tag" --json isDraft,isPrerelease,tagName,assets |
            jq -e --arg tag "$tag" --arg channel "$channel" '
                (.isDraft == false) and .tagName == $tag and .isPrerelease == ($channel == "deps") and
                (([.assets[].name] | sort) == (if $channel == "stable"
                    then ["SHA256SUMS", "Tenka-x86_64.AppImage", "Tenka-x86_64.AppImage.zsync"]
                    else ["SHA256SUMS", "Tenka-x86_64.AppImage"] end))
            '
    )
    if [[ "$channel" == stable ]]; then
        [[ $(gh api "repos/$GITHUB_REPOSITORY/releases/latest" --jq .tag_name) == "$tag" ]]
    fi
}

sync_branch() {
    local branch=$1
    fetch
    if ! git rev-parse --verify --quiet "origin/$branch" >/dev/null; then
        git push origin "origin/main:refs/heads/$branch"
        return
    fi
    if git merge-base --is-ancestor origin/main "origin/$branch"; then
        return
    fi
    git checkout --detach "origin/$branch"
    git config user.name 'github-actions[bot]'
    git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
    if ! git merge --no-edit origin/main; then
        local conflicts
        conflicts=$(git diff --name-only --diff-filter=U)
        if [[ "$branch" != deps || "$conflicts" != VERSION ]]; then
            echo "Conflicts while syncing $branch with main: $conflicts" >&2
            git merge --abort
            return 1
        fi
        git checkout origin/main -- VERSION
        git commit --no-edit
    fi
    git push origin "HEAD:refs/heads/$branch"
}

case "$1" in
    prepare) prepare "$2" ;;
    publish) publish "$2" "$3" ;;
    sync) sync_branch "$2" ;;
    *) echo "Unknown release command: $1" >&2; exit 1 ;;
esac
