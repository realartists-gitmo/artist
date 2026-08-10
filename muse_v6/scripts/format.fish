#!/usr/bin/env fish

function main
    set root (realpath (dirname (status filename))/..)
    cd $root; or return 1
    command cargo +1.97.1 fmt --all
end

main
