.PHONY: run run-server build build-server build-spooky clean

run:
	cargo run --bin spooky

run-server:
	cargo run --bin server

build:
	cargo build --release

build-server:
	cargo build --bin server --release

build-spooky:
	cargo build --bin spooky --release

clean:
	rm -f target/release/spooky
	rm -f target/release/server