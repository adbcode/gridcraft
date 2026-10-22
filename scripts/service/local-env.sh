#!/bin/sh
# Keep credentials out of shell evaluation, command output and tracked files.
set -eu

service_repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$service_repo_root"

if [ -L .env ]; then
    printf '%s\n' 'Refusing to change a symbolic-link .env.' >&2
    exit 1
fi

if [ -e .env ]; then
    if [ ! -f .env ]; then
        printf '%s\n' '.env exists but is not a regular file.' >&2
        exit 1
    fi
    chmod 600 .env
    printf '%s\n' 'Preserved existing .env values; permissions are 600.'
else
    # noclobber also protects a file created after the existence check.
    (umask 077; set -C; cat .env.example > .env)
    printf '%s\n' 'Created .env with permissions 600. Add credentials in your editor.'
fi
