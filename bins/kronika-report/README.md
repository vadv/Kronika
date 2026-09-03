# kronika-report

`kronika-report <signed-decimal>.zms <output>.html` reads exactly one finished
standalone ZMS and writes one self-contained HTML report. The input filename is
the canonical signed-decimal segment identity followed by `.zms`.

The generated file embeds the production report UI, its browser query module,
the source ZMS, and the canonical isolated IDX derived from that ZMS. It has no
server, external sidecar, authentication, or network dependency.
