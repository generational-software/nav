# Navigator

This is a toy, an exploration, a bit of fun.

`nav` is a directory navigator for Wayland. It draws the current directory as a plaza of standing stones: one stone for each folder, smaller stones for the folders inside those, and a taller amber stone for the parent. Files are left out.

Scanning runs on background threads. Stones appear as soon as a folder is found, so you can orbit and click while a large directory is still being read.

## Requirements

- Linux.
- A Wayland session. `WAYLAND_DISPLAY` names the compositor. If that is unset and `DISPLAY` is set, the window opens on X11 instead.
- A GPU that can draw the window. The picture is wgpu 30, with shaders written in WGSL. On Linux the usual backend is Vulkan.
- A sans-serif font that fontconfig can see, such as DejaVu Sans or Noto Sans. Folder names come from that list.
- Rust 1.90 or newer, and Cargo, to build it.
- bash, if you want the commands on the ring to run. Browsing works without it.
- A sound device, only if you start nav with `-s`. Without that flag it stays quiet.

## Run

From this directory:

```
cargo run --bin nav
cargo run --bin nav -- ~/somewhere
cargo run --bin nav -- -s
cargo run --bin nav -- -d
cargo run --bin nav -- --floor hex
```

With no path, `nav` opens the working directory. To put the command on your `PATH`:

```
cargo install --path .
```

## Look around

Drag to orbit the plaza. Scroll to zoom. Click a stone to open that folder. Click the amber stone, or press Backspace, to go up. The mouse back button goes up too. Home resets the camera. R reads the folder again. Escape closes the window. H shows or hides folders whose names start with a dot. They stay hidden until you pass `-d` or press H.

Pass `-s` and a click plays a short tap. Opening a folder, going up, and clicking empty ground each sound a little different. Moving the pointer onto a folder plays a softer tap. Moving onto a command plays a different chime. The same stone keeps its tone. Without `-s`, nav stays quiet.

The floor is a fine grid. `--floor hex` draws a hexagonal pattern instead, and `--floor checker` draws a muted checkerboard.

A link to a folder is shown, and it is not walked until you click it.

## Tags

A folder may contain a `.nav` file. Nav reads it before drawing that folder. The file is JSON:

```json
{
  "colour": "red",
  "shape": "cone"
}
```

`colour` is a name (`red`, `teal`, `amber`, …) or a hex value such as `#c45a2a`. `shape` is one of `prism`, `cylinder`, `box`, `cone`, `pyramid`, `crystal`, `obelisk`. Without a file, a folder is the usual tapered prism.

To write the file in the working directory:

```
nav --tag red cone
```

Press R after changing a `.nav` file and the stone updates.

## Commands

If the open folder contains a `Cargo.toml` or a `package.json`, each command in those files is a thicker segment of the center ring. Cargo bins and examples come first. When a package lists no bins, the package name stands in for `cargo run`. npm scripts follow, in the order they are written. While a command from this folder is running, its segment glows and stands taller than the others.

Click a segment to run that command in a login bash in that folder, so it picks up the same environment as a terminal. A tile on the left shows its name and the latest output. When the command exits, a Finished tag sits at the bottom of the tile. Away from the folder it was started in, the title also names that folder. The arrow on the tile comes back to that folder. The × stops the command, and once it has stopped the × removes the tile. Several commands can run at once, and their tiles stack down the left side. Press R after editing either file.

## Help

`nav --help` prints:

```
usage: nav [-d] [-s] [--floor grid|hex|checker] [directory]
       nav --tag <colour> <shape>
       nav -h
       nav --help

Open a 3D view of a directory. With no path, nav uses the working directory.
-d shows folders whose names start with a dot. Without it they stay hidden. H toggles them in the window.
-s turns on click sounds.
--floor picks the plaza floor. grid is a fine line grid and the default. hex is a hexagonal pattern. checker is a muted checkerboard.
-h and --help print this text.

Before a folder is drawn, nav reads <folder>/.nav if it is there.
The JSON keys are colour and shape. --tag writes that file in the working directory.
A Cargo.toml or package.json in the open folder puts its commands on the center ring as thicker segments. While a command is running, its segment glows and stands taller.
Click a segment to run that command in a login bash. A tile on the left shows its output. When it exits, Finished sits at the bottom of the tile.
With -s, a click plays a short tap. Opening a folder, going up, and clicking empty ground sound different.
Moving the pointer onto a folder plays a softer tap. A command plays a different chime.
colours: amber, teal, red, green, blue, yellow, orange, purple, pink, white, black, grey, gray, brown, gold, cyan, lime, navy, violet, coral, mint, sand, copper, rose
shapes: prism, cylinder, box, cone, pyramid, crystal, obelisk
colour may also be a hex value such as #c45a2a.
```
