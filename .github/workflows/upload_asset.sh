#!/bin/bash

# Assure parameters are correct.
if [ $# -lt 2 ]; then
    echo "Usage: upload_asset.sh <FILE> <TOKEN>"
    exit 1
fi

repo="an0nn30/rusty_conch"
file_path=$1
bearer=$2

echo "Starting asset upload from $file_path to $repo."

# Get the release for this tag.
tag="$(git describe --tags --abbrev=0)"

# Make sure the git tag could be determined.
if [ -z "$tag" ]; then
    printf "\e[31mError: Unable to find git tag\e[0m\n"
    exit 1
fi

echo "Git tag: $tag"

# Fetch upload URL and release ID for a given tag.
# Since this might be a draft release, we can't just use the /releases/tags/:tag
# endpoint which only shows published releases.
fetch_release_info() {
    local json
    json=$(\
        curl -s \
            --http1.1 \
            -H "Authorization: Bearer $bearer" \
            "https://api.github.com/repos/$repo/releases" \
    )

    upload_url=$(\
        echo "$json" \
        | grep -E "(upload_url|tag_name)" \
        | paste - - \
        | grep -e "tag_name\": \"$tag\"" \
        | head -n 1 \
        | sed 's/.*\(https.*assets\).*/\1/' \
    )

    release_id=$(\
        echo "$json" \
        | grep -E "(\"id\":|tag_name)" \
        | paste - - \
        | grep -e "tag_name\": \"$tag\"" \
        | head -n 1 \
        | sed 's/.*"id": \([0-9]*\).*/\1/' \
    )
}

echo "Checking for existing release..."
fetch_release_info

# Create a new release if we didn't find one for this tag.
if [ -z "$upload_url" ]; then
    echo "No release found. Creating new release..."

    response=$(
        curl -f -s \
            --http1.1 \
            -X POST \
            -H "Authorization: Bearer $bearer" \
            -d "{\"tag_name\":\"$tag\",\"draft\":true}" \
            "https://api.github.com/repos/$repo/releases" \
            2> /dev/null
    )

    if [ $? -eq 0 ]; then
        # We created it — extract URL and ID from the response.
        upload_url=$(\
            echo "$response" \
            | grep "upload_url" \
            | sed 's/.*: "\(.*\){.*/\1/' \
        )
        release_id=$(\
            echo "$response" \
            | grep '"id":' \
            | head -n 1 \
            | sed 's/.*"id": \([0-9]*\).*/\1/' \
        )
    else
        # Creation failed — another job likely created it first.
        # Wait briefly and re-fetch.
        echo "Release creation failed (likely race condition). Retrying fetch..."
        sleep 3
        fetch_release_info
    fi
fi

# Propagate error if no URL for asset upload could be found.
if [ -z "$upload_url" ]; then
    printf "\e[31mError: Unable to find release upload url.\e[0m\n"
    exit 2
fi

echo "Release found (id: $release_id)."

# Delete existing asset with the same name (if any) to allow re-upload.
file_name=${file_path##*/}
if [ -n "$release_id" ]; then
    existing_asset_id=$(\
        curl -s \
            --http1.1 \
            -H "Authorization: Bearer $bearer" \
            "https://api.github.com/repos/$repo/releases/$release_id/assets" \
        | grep -B 2 "\"name\": \"$file_name\"" \
        | grep '"id":' \
        | sed 's/.*"id": \([0-9]*\).*/\1/' \
    )
    if [ -n "$existing_asset_id" ]; then
        echo "Deleting existing asset $file_name (id: $existing_asset_id)..."
        curl -f -s \
            --http1.1 \
            -X DELETE \
            -H "Authorization: Bearer $bearer" \
            "https://api.github.com/repos/$repo/releases/assets/$existing_asset_id" \
            > /dev/null
    fi
fi

# Upload the file to the tag's release. Retry once on failure.
upload_asset() {
    echo "Uploading asset $file_name to $upload_url..."
    curl -f -s \
        --http1.1 \
        -X POST \
        -H "Authorization: Bearer $bearer" \
        -H "Content-Type: application/octet-stream" \
        --data-binary @"$file_path" \
        "$upload_url?name=$file_name" \
        > /dev/null
}

if ! upload_asset; then
    echo "Upload failed, retrying in 5 seconds..."
    sleep 5
    if ! upload_asset; then
        printf "\e[31mError: Unable to upload asset.\e[0m\n"
        exit 3
    fi
fi

printf "\e[32mSuccess\e[0m\n"
