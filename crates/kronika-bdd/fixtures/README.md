# BDD fixtures

Each directory here is a procfs root a scenario points `KRONIKA_PROC_ROOT` at.
The files are the real thing a scenario reads, so what a fixture does and does
not provide is visible in a diff.

`procfs-without-meminfo` holds the minimum the collector needs to start and
write a segment. `meminfo` and `vmstat` are deliberately absent.
