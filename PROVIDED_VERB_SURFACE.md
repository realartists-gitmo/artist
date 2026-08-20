write(target, content) -- create or overwrite a file
move(source, target) -- move a file, can also naturally rename or delete with no target
read(target) -- read a file
edit(target, im unsure) -- edit a file
find(fuck idk) -- find a file, fff-backed, fuzzy first, prefixes for lit: or rg: regex.
grep(fuck idk) -- grep files, fff-backed, fuzzy first, prefixes for lit: or rg: regex.

things im sus about---though i am also sus about the above
poll(target, match?, timeout?)
send(target, content) -- webGPT said sending to a live session like a terminal or other input accepting thing could be conceptualized as writing to stdin, but im unsure if thats ergonomic, i also dont know how we'd represent stdin
run(uri?, target?) -- run an executable and return the provided uri if not specified. some resources have default executable behavior (like bash:// or python://, so running a uri within them that isnt already assigned provisions and creates the file)
abort(uri) -- kill a process?

how to send signals...?
is this abstraction leaky...?
are we missing anything...?

side note: this entire tool surface should use teca anchors, not line numbers, with the 'teca' crate.
