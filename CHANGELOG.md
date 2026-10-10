# Changelog

Newest entries first. Add a dated line when the app changes in a way a later reader would want to know.

- 2026-10-10 While a command is running, its ring segment glows and stands taller than the others.
- 2026-10-10 When a command exits, its tile gains a Finished tag at the bottom of the frame.
- 2026-10-10 If the Wayland display, the GPU, or a font is missing at startup, nav says what is missing and exits. If bash is not on PATH, it says so and still opens.
- 2026-10-10 -h and --help both print the usage text. Dot folders start shown when nav is started with -d.
- 2026-10-10 Away from a command's folder, its tile title includes that folder's name.
- 2026-10-10 Moving the pointer onto a command plays a different chime than moving onto a folder.
- 2026-10-10 The floor lines are finer. `--floor` chooses that grid, a hex pattern, or a muted checkerboard.
- 2026-10-10 A command runs in a login bash, so it sees the same environment as a terminal.
- 2026-10-10 Dot folders stay hidden until nav is started with -h or H is pressed. `nav --help` prints how to run it.
- 2026-10-10 Click sounds play only when nav is started with -s.
- 2026-10-10 Clicking a command on the center ring runs it. Each one gets a tile on the left with its output, an arrow back to the folder, and an × to stop it.
- 2026-10-10 Commands are thicker segments of the center ring, one arc for each command.
- 2026-10-10 H shows or hides folders whose names start with a dot.
- 2026-10-10 Escape closes the window.
- 2026-10-10 Moving the pointer onto a folder plays a softer tap.
- 2026-10-10 A click plays a short tap. Opening a folder, going up, and clicking empty ground each sound different.
- 2026-10-10 Commands from a Cargo.toml or package.json in the open folder appear as blobs on the center ring.
- 2026-10-10 A folder can carry a .nav file that sets its stone's colour and shape. `nav --tag` writes that file. Directory names sit on a dark rectangle so they stay readable.
- 2026-10-10 The open folder's name sits in the middle of the center ring.
- 2026-10-09 Subfolders drawn around a folder sit far enough apart to tell two of them apart.
- 2026-10-09 Navigator opens a 3D view of the working directory. Folders appear as they are found, and a click moves into that folder.
