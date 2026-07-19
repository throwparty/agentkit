set shell := ["bash", "-eux", "-o", "pipefail", "-c"]

default: list

list:
    just --list --justfile {{source_directory()}}/mod.just

start:
    cd {{source_directory()}} && yarn start

build:
    cd {{source_directory()}} && yarn build

serve:
    cd {{source_directory()}} && yarn serve

clear:
    cd {{source_directory()}} && yarn clear

typecheck:
    cd {{source_directory()}} && yarn typecheck
