# Record an authentic terminal GIF of a real noworries run.
#
#   1. Install VHS:  https://github.com/charmbracelet/vhs  (brew install vhs)
#   2. From the repo root, in a project that has a noworries.yml:
#        vhs demo/demo.tape
#   3. It writes docs/demo.gif — then point the README <img> at docs/demo.gif
#      instead of docs/demo.svg.
#
# The static docs/demo.svg ships as the default so the README renders a hero
# image out of the box, with no recording step required.

Output docs/demo.gif

Set Shell "bash"
Set FontSize 18
Set Width 1200
Set Height 720
Set Padding 22
Set Theme "Dracula"

Type "noworries changed"
Sleep 600ms
Enter

# Let the real run play out: infra up -> app start -> checks -> READY -> teardown.
Sleep 8s
