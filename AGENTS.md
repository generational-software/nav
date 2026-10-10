# Navigator

Follow the lab rules in [../AGENTS.md](../AGENTS.md). Do not copy them into this file.

Navigator is a native Wayland program, started with `nav`. It is not a browser app, so it does not use the studio theme picker or language picker.

If startup cannot open the Wayland display, the GPU, or a font, or the GPU rejects a shader, nav names what failed and exits. A missing bash is reported and the window still opens.

The plaza floor is a fine grid. `--floor hex` draws a hexagonal pattern, and `--floor checker` draws a muted checkerboard. The default is `grid`.

The scanner runs on background threads and lists directories only. The window shows the parent folder, the folders in the current directory, and one level of their subfolders. A click navigates, and the view updates before the scan finishes. Folders whose names start with a dot stay hidden until `nav` is started with `-d` or H is pressed. H toggles them. The scan keeps them either way. `-h` and `nav --help` both print the usage text.

Before a folder is drawn, nav reads `<folder>/.nav` when that file exists. It is JSON with `colour` and `shape`. `nav --tag <colour> <shape>` writes the file in the working directory. Directory names are drawn on a dark rectangle.

When the open folder contains `Cargo.toml` or `package.json`, the commands in those files are drawn as thicker segments of the center ring. Clicking a segment runs that command in a login bash in the open folder, so it sees the same environment as a terminal. While that command is running, its segment glows and stands taller. Each command gets a tile on the left: the name, the latest output, an arrow back to the folder, and an × that stops it. When the command exits, Finished sits at the bottom of the tile. Away from that folder, the title also names it. Tiles stack, and they stay when you move to another folder.

Click sounds are off unless nav is started with `-s`. With that flag, a click plays a short synthesized tap. Opening a folder, going up, and clicking empty ground use different taps. Moving the pointer onto a folder plays a softer tap. Moving onto a command plays a different chime. A drag does not.
