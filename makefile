BUILDTIME := $(shell date '+%Y-%m-%d | %H:%M:%S')
REPONAME := github.com/Purrquinox/Chlamydia-RWSGS
PROJECTNAME := chlamydia_rwsgs

COMBOS := linux/amd64 linux/arm64 windows/amd64 windows/arm64

.PHONY: all release fmt clean prerelease

all:
	@echo "Building in debug mode..."
	cargo build

release:
	@python build_release.py

fmt:
	@echo "Formatting Rust code..."
	cargo fmt --all

clean:
	@echo "Cleaning up..."
	rm -rf bin
	rm -rf target
prerelease:
	@echo "Installing prerequisites..."
	sudo apt-get update
	sudo apt-get install -y gcc-multilib g++-multilib