# Navigator

`nav` is a directory navigator for Wayland. It draws the current directory as a plaza of standing stones: one stone for each folder, smaller stones for the folders inside those, and a taller amber stone for the parent. Files are left out.

Scanning runs on background threads. Stones appear as soon as a folder is found, so you can orbit and click while a large directory is still being read.

## Run

From this directory:

```
cargo run --bin nav
cargo run --bin nav -- ~/somewhere
```

With no path, `nav` opens the working directory. To put the command on your `PATH`:

```
cargo install --path .
```

## Look around

Drag to orbit the plaza. Scroll to zoom. Click a stone to open that folder. Click the amber stone, or press Backspace, to go up. The mouse back button goes up too. Home resets the camera. R reads the folder again. Escape closes the window. H shows or hides folders whose names start with a dot. They are shown until you hide them.

A click plays a short tap. Opening a folder, going up, and clicking empty ground each sound a little different. Moving the pointer onto a folder plays a softer tap. The same stone keeps its tone.

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

If the open folder contains a `Cargo.toml` or a `package.json`, each command in those files is a blob on the center ring. Cargo bins and examples come first. When a package lists no bins, the package name stands in for `cargo run`. npm scripts follow, in the order they are written. The blobs are labels only. Press R after editing either file.
