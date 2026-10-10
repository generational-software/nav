# Navigator

Follow the lab rules in [../AGENTS.md](../AGENTS.md). Do not copy them into this file.

Navigator is a native Wayland program, started with `nav`. It is not a browser app, so it does not use the studio theme picker or language picker.

The scanner runs on background threads and lists directories only. The window shows the parent folder, the folders in the current directory, and one level of their subfolders. A click navigates, and the view updates before the scan finishes. H toggles folders whose names start with a dot. The scan keeps them either way.

Before a folder is drawn, nav reads `<folder>/.nav` when that file exists. It is JSON with `colour` and `shape`. `nav --tag <colour> <shape>` writes the file in the working directory. Directory names are drawn on a dark rectangle.

When the open folder contains `Cargo.toml` or `package.json`, the commands in those files are drawn as blobs on the center ring. The blobs are not clickable.

A click plays a short synthesized tap. Opening a folder, going up, and clicking empty ground use different taps. Moving the pointer onto a folder plays a softer tap. A drag does not.
