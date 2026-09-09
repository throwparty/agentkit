#!/usr/bin/env bash
for file in "$@"; do
  just --justfile "$file" --fmt
done
