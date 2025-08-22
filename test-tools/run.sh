#!/bin/sh

set -eu

x() {
	echo + "$@"
	"$@"
}

run() {
	p=$1
	shift
	x cargo build-sbf "$@" -- -p alloc-$p

	# I have target-dir configured globally to be in ~/.cache/cargo-target;
	# if `target` directory doesn’t exist it’s probably there.
	if [ -d target ]; then
		target_dir=target
	elif target_dir=$HOME/.cache/cargo-target; ! [ -d "$target_dir" ]; then
		echo 'unable to locate target-dir' >&2
		return 1
	fi

	x solana program deploy "$target_dir/deploy/alloc_$p.so"

	sleep 1
	x cargo run -rp client -- -f "$target_dir/deploy/alloc_$p-keypair.json"
}

run_perf() {
	for alloc in '' bump-alloc ll-alloc; do
		for cnt in '' small-count; do
			for free in '' free rev-free; do
				flags=$alloc
				[ -z "$free" ] || flags=$flags${flags:+,}$free
				[ -z "$cnt" ] || flags=$flags${flags:+,}$cnt
				run perf ${flags:+--features} $flags
			done
		done
	done | tee logs

	echo
	grep ^Program\ log: logs |grep first: |uniq
	grep ^Program\ log: logs |grep -v first:
}

case "$#:${1:-}" in
1:--perf|1:-p)
	run_perf
	;;
1:--test|1:-t)
	run test
	run test --features test-global
	;;
*)
	echo 'usage: $0 ( -p | --perf | -t | --test )' >&2
	exit 1
esac
