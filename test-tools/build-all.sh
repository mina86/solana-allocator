#!/bin/sh

set -eu

build() {
	pkg=$1; features=
	shift
	for arg; do
		features=$features${features:+,}$arg
	done
	set -- cargo build-sbf -- -p "$pkg" ${features:+--features} $features
	echo + "$@"
	"$@"
}

for alloc in '' bump-alloc ll-alloc; do
	for cnt in '' small-count; do
		for free in '' free rev-free; do
			build alloc-perf $alloc $cnt $free
		done
	done
done

for feature in '' test-global; do
	build alloc-test $feature
done

for internal in '' internal; do
	for global in '' global; do
		build nop-test $internal $global
	done
done
