# Start-menu icon sources

The two SVG files are the 480 px exports supplied by the user from icons8:

- `windows-11.svg` — `icons8-视窗11-color.zip`
- `windows-10.svg` — `icons8-windows-10-color.zip`

They are 48×48 viewBox artwork. `dock-render::Renderer::draw_start_widget`
reproduces their exact path geometry and colors in code so the dock does not
need a runtime SVG renderer:

- Windows 11: `#1976D2`, four axis-aligned rectangles.
- Windows 10: `#00B0FF`, four slanted panes.

Keep these files as the visual source of truth when adjusting the artwork.
