# Stijl User Guide

A complete description of Stijl’s features. For downloads and building, see the
[README](../README.md).

Keyboard shortcuts are written for Linux and Windows; on a Mac, use ⌘ instead of Ctrl.
All shortcuts can be changed under *Preferences › Keyboard Shortcuts* (Ctrl ?).

## Drawing

- **Tools (floating bar at the top left of the canvas):** Select (V), Edit Nodes (A), Shapes (R: rectangle, ellipse,
  polygon, star), Shape Builder (Shift M or K), Pen (P) / Curvature Pen (Shift P),
  Freehand (N), Eraser (Shift E) / Scissors (C), Text (T) / Text on Path (Shift T) /
  Text Box, Guides (G). Hover over a tool to see its name and shortcut. Pan the view with
  Space + drag, the middle mouse button or the scroll wheel
- **Tool groups:** tools with variants (shapes, pens, eraser/scissors, text) have a small
  arrow next to their symbol. Clicking one opens its group in the bar, so all variants can be chosen with one click. The group
  closes again as soon as you pick another tool and keeps the variant you used last.
  Pressing the shortcut of the active tool again steps to the next variant (R, R, R …)
- **Tool options:** tools with settings show a second small bar under the tools – corners
  for polygons and stars, size for the eraser, smoothing and closing distance for freehand
- **Pen / Curvature Pen:** hold Shift to snap the direction to 45° steps and at right angles
  to the previous segment; a point only becomes smooth once you drag noticeably. The
  Curvature Pen always shows the line exactly as the next click will make it – over the
  starting point already as the closed shape; a click closes it. Wherever a short hold shows
  a preview (placing an image or text into a shape, dropping a library item onto an object),
  the same filling ring appears
- **Clicking vs. dragging:** small jitter while clicking (up to 5 pixels) is still a click –
  nothing moves and no handles or curves appear by accident
- **Freehand:** adjustable smoothing (tool options); if start and end are close together,
  the shape is closed
- **Edit Nodes:** click and drag nodes, select several with Shift or a selection frame; drag
  the round handle to round corners – also on rectangles and ellipses. Works on **several
  objects** at once: a frame dragged from empty space picks up the nodes of all objects
  inside it, and Shift + click on another object adds it with all its nodes (click again to
  remove it). Moving, rounding, corner/smooth and deleting apply to all selected nodes
- **Polygons and stars:** the number of corners is set in the tool options and can be
  changed later in the properties
  (position, size, rotation and corner radius are kept)
- **Corner radius:** drag the round handles in the Edit Nodes tool – one corner or several
  together. The handle always stays at the same distance from the edge of the rounding and
  is limited by the space the neighbouring edges leave. The radius is shown next to the
  pointer while dragging (and the angle while rotating)
- **Eraser:** removes area from filled shapes and cuts lines apart; set the size in the
  tool options or with [ and ]. With a selection it only affects the selected objects
- **Scissors (C):** in the group with the Eraser. Drag a line across objects
  to cut them: filled shapes are split into pieces (curves stay curves) and lines are cut at
  the crossing points; every piece becomes an object of its own. A cut that does not go all
  the way through changes nothing. With a selection only the selected objects are cut
- **Shape Builder:** all outlines form areas – including gaps between shapes and areas that
  are only bounded by open lines (e.g. freehand strokes); curves stay curves. The area under
  the pointer is highlighted as you hover. With a selection only the selected objects count
  (the whole network of areas is then shown dashed), otherwise everything visible.
  - **Click** an area: it becomes a new shape in the current fill colour (existing shapes
    stay unchanged)
  - **Drag** across several areas: they are merged into one shape (in the colour of the area
    where you started); the shapes involved fall apart into their remaining areas, open
    lines are kept
  - **Alt** while clicking or dragging: the areas are deleted from the shapes
- **Path Operations:** Unite, Subtract Front, Intersect, Exclude Overlap – as icons in the
  floating bar when two or more objects are selected, and in the right-click menu
- **Join Paths** (right-click or Ctrl J): open paths are joined at their nearest ends – small
  gaps (under 4 screen pixels) merge into one point, larger ones are bridged with a straight
  segment. A single open path is closed

## Text

- **Edit directly in the document** (including text on a path): a click places the cursor,
  dragging or double-clicking selects, Esc finishes
- **Text options:** a collapsible card of its own (when collapsed, font and size are shown
  in its header), grouped into font, character, paragraph, text box and text on path – each
  group only appears where it applies. At the bottom are the text styles of the current
  library (like the library colours for fill and stroke); the “+” after them saves the style
  of the selected text to the library. The font is chosen from a
  searchable drop-down, each font shown in its own typeface; below it the **style** (all
  styles of the family, e.g. Light, Regular, Medium, Semi Bold, Extra Bold, Black – each also
  in italics), the size and, for **variable fonts**, a weight slider (100–900). Underline,
  strikethrough, **small caps** and **ligatures** are icon buttons in one row under
  *Character*. *Convert Text to Paths* is here as well –
  every letter becomes a path of its own and all of them form a group (Ctrl Shift G ungroups
  it so that single letters can be moved)
- **Formatting like a word processor:** in text boxes, font, size and styles apply only to
  the selected text; with a cursor and no selection, to what you type next; if the box
  itself is selected, to the whole box. If a box contains different fonts or sizes, the
  fields stay empty – a choice then applies to everything
- **Paragraphs:** left, right, centred, justified; text boxes also top/middle/bottom; line
  and letter spacing. Enter starts a new paragraph (with adjustable paragraph spacing),
  Shift+Enter only a new line. Tab inserts a tab (stops every 2.4 font sizes). Text boxes
  have an adjustable number of **columns** (1–8): text flows into the next column once the
  box height is reached. Ligatures (fi, fl …) and small caps can be switched per text
- **Lists and indents** (in text boxes, per paragraph): a button toggles the list, the small
  arrow next to it chooses bullets, squares, dashes or numbers; increase/decrease indent
  moves paragraphs in steps (indented numbers count separately). Bullets and numbers take
  the size of their paragraph's text. While editing this applies to the selected paragraphs,
  otherwise to all
- **Text on Path:** above or below the line and in reverse direction (e.g. readable along the
  bottom of a circle); on closed paths the text continues past the start
- **Text Box:** filled with selected placeholder text when you draw it – just start typing
- **Text in shapes:** drag a text box onto a shape and hold briefly – the text wraps inside
  the shape
- **Background and frame** for all kinds of text (text box, text, text on path): frame fill,
  frame stroke and padding. When text is scaled, the padding grows or shrinks with the font
  size

## Appearance

- **Layout of the properties:** like the GNOME settings, every section is a card – name on
  the left, control on the right, all controls the same width. Related settings form a
  group; only groups are separated by lines. Fill, stroke, shadow and effects are rows of
  one card that expand
- **Fill and stroke:** collapsible – when collapsed, the opacity (and for the stroke also the
  width) and a circle in the current colour are shown; only one section is open at a time.
  When expanded: the *Colour* / *Gradient* tabs with the **eyedropper** next to them (pick a
  colour from the document or a photo: a ring around the pointer shows the colour, a click
  takes it, Esc cancels), colour field, hue, opacity (slider with number field), hex value and
  the library colours; “+” adds the current colour to a library of your choice. With several
  objects, the opacity slider only changes the opacity – every object keeps its colour
- **Stroke:** width, position (centre, inside, outside), corners, ends and line ends in the
  expanded section; *Convert Stroke to Fill* turns the stroke into an object of its own that
  looks exactly like the stroke – same corners, ends, dashes and line ends. Dash patterns: solid, dashed, dotted, dash-dot or long
  dashes (lengths follow the stroke width; also imported from SVG and Illustrator files);
  corners mitred, round or bevelled; ends butt, round or square; line ends (arrow, dot, bar)
- **Images:** opacity and **temperature** under *Fill* (−100 % cold, 0 % original, 100 %
  warm), frame line under *Stroke*
- **Text:** frame fill and frame stroke are collapsible like fill and stroke; the padding is
  in the text options
- **Gradients:** for fill, stroke, shadow and text, linear or radial; a gradient bar with
  round colour stops – click to add one, drag to move, Del to remove; adjustable angle;
  gradients can be saved to the library like colours; the gradient line can be dragged in the
  document
- **Patterns** (not for text, where they would be unreadable; a tab next to *Colour* and
  *Gradient* for the fill): chequerboard, dots or stripes as previews, two colours of your
  choice, size and angle; saved in SVG as a real pattern
- **Effects:** the *Effects* button in the floating bar works on all selected objects at once;
  submenus open to the side. One effect per category is possible, categories can be
  combined, and the strength is adjustable (except for the colour overlay):
  - **Colour overlay:** Lighten or Darken (default is Cover). Lighten mixes like spotlights:
    a single object shows its colour, consecutive Lighten objects add their light, weighted by
    opacity (red + green = yellow, red + green + blue = white). Darken mixes like layers of
    coloured glass towards black
  - **Texture:** **Frosted glass** – what lies underneath appears blurred; **Brushed metal** –
    fine horizontal lines with a metallic sheen; **Gloss** – a soft highlight at the top, a
    bright edge and a slight reflection at the bottom
  - **Artistic effects:** grain, pixels, watercolour – on the fill of shapes and text and also
    on images. They only change colours, never opacity
  - **3D effects:** give the object a relief with light (from the top left) and shadow; with
    transparency, what lies underneath is distorted as if seen through glass. **Depth** runs
    from −100 % (pressed in) through 0 % (flat) to +100 % (raised); **Lighting** (0–100 %,
    default 50 %) sets how bright the light and how dark the shadow are. Shapes:
    **Raindrop** (flattened sphere), **Sphere** (fully round), **Crystal** (tapering with a
    flat top), **Pyramid** (fully pointed). Can be combined with a texture; not available for
    images
  - **Image filters** (images only): sepia, black & white, invert colours
  - **Extras:** shadow or reflection below the object

  Every effect applied appears in the properties as a collapsible section with its type and
  strength in the header, where it can also be removed. All sliders look the same (a round
  bar like the colour opacity) and have a field next to them for typing the number
- **Trace image:** if an image consists of flat colour areas (a logo, an illustration), the
  floating bar offers *Trace*. The number of colours is adjustable; the image is replaced by
  grouped, editable vector areas (can be undone). Not offered for photos
- **Images:** insert (Ctrl I) or drag onto the canvas; drag onto a shape and hold briefly to
  place the image inside the shape

## Arranging

- **Floating bar:** above (or, if there is little room, below) the selection – it only shows
  what fits the selection: lock/unlock, effects and “…” for more (the same menu as the
  right-click menu; submenus open to the side, nothing needs scrolling). With several
  objects *Group*, with groups *Ungroup*; with two or more objects *Align* (to each other)
  and the four **path operations** as icons; with three or more objects or groups the
  align menu also offers *Distribute* (horizontally or vertically – the outer ones stay in
  place, all gaps become equal)
- **Right-click:** cut, copy, paste, duplicate, scale, arrange, path operations, *Blend …*
  (with exactly two shapes: in-between shapes with an adjustable number of steps – shape,
  colour, stroke width and opacity are blended; both shapes and the in-between shapes are
  grouped), move to layer, add to library (asks: colour, style or graphic) and delete
- **Groups:** Ctrl G groups the selection, Ctrl Shift G ungroups it; a click always selects
  the whole group; groups are saved as `<g>`, and groups from other SVG files are kept
- **Rotated objects:** the selection frame rotates with them; width and height can be
  changed along the object’s own axes
- **Lock:** locked objects can be selected and recoloured but not moved, rotated, scaled or
  deleted
- **Smart guides:** snap to corners, centres, anchor points and paths of other objects, to
  artboards, guides and (optionally) the grid
- **Equal spacing:** while moving, the selection snaps when its gap to a neighbour equals
  another gap in the same row (so an even row can be continued) or when the space on the
  left and right is equal; can be turned off in the preferences
- **Guides:** click and drag – the direction of the drag sets the angle (Shift: 45° steps);
  can also be dragged out of the rulers. They snap to anchor points, corners, centres and
  edges when created and when moved later
- **Align:** left, centre, right, top, middle, bottom. A single object aligns to the artboard
  it is on. In the *Position* section several objects (groups count as one) align as a whole
  to the page; *Align* in the floating bar aligns them to each other – to their common
  bounding box
- **Scale …** (right-click): exact percentages, uniform or width/height separately,
  optionally including strokes and shadows
- **Copying between documents:** Ctrl C in one tab, Ctrl V in another – at the same position
- **Copy as Image** (right-click; a shortcut can be set under *Preferences › Keyboard
  Shortcuts*): the selection – or the artboard if nothing is
  selected – as an image on the clipboard, ready to paste into a chat, email or document

## Artboards and layers

- **“Layers and Artboards” panel:** layers at the top, artboards below a divider, each with
  its own “+” button
- **Several artboards** in one file: each has its own row with a preview, a gear button
  (size, name), **duplicate** (copies the artboard with everything on it and places it to the
  right) and delete – any artboard can be deleted while another one remains (if the first
  goes, the next one takes its place). While the panel is open, artboards can be edited: dragging the name
  moves an artboard (everything lying fully on it moves along), the handles resize it. Both
  snap to other artboards, objects and guides like objects do; Shift keeps the movement
  horizontal or vertical
- **Layers:** a large preview showing only that layer’s objects – on all artboards and
  beyond, zoomed so that everything in the file fits (with several artboards their outlines
  are hinted) – a stripe in the layer colour on the left, and lock, show/hide and delete on
  the right; reorder by dragging (a line shows where it will go, the other rows already move
  aside); double-click the preview to rename.
  If the selection is on a layer, a small square in the layer colour appears at the bottom
  right of its preview – drag it onto another layer to move the selection there
- **Guides belong to layers:** new guides go on the current layer and are hidden, locked or
  deleted together with it
- **Export:** PNG, JPG, PDF, SVG. *Export Selection…* (menu) or *Selection only* in the
  dialog exports only the selected objects, cropped to the selection. With several
  artboards you pick them by preview (All/None) – PDF gets one page per artboard, the other
  formats one file per artboard. For PDF and SVG you choose the **effect quality** (small
  150 dpi, medium 300 dpi, high 600 dpi)
  - **PDF:** searchable text (fonts are embedded) or text converted to outlines; colours
    **RGB**, **CMYK** or **PDF/X-4** for print shops (with the embedded colour profile of the
    printing condition, trim and bleed boxes); adjustable **bleed** (3 mm preset for PDF/X).
    Stijl looks for printing profiles in the usual folders (e.g. “Coated FOGRA39” from
    colord); others such as “PSO Coated v3” are available free of charge from the ECI
    (eci.org) and can be chosen in the dialog. Neutral greys and black are printed with
    black (K) only. Text on a path is exported as outlines
  - **SVG:** for other applications – objects with Stijl effects (glass, 3D, textures,
    artistic effects, reflection, image filters) are embedded as images, text boxes are
    written as fixed lines (wrapping, columns, justification and text in shapes look the same
    everywhere); fonts can be embedded

## Library

- The *Library* button at the top left opens it in the left sidebar; your own libraries with
  colours, styles and text styles
- Included: the default library “Stijl” with basic colours and the text styles Heading, Text
  and Quote – it cannot be changed; anything you add goes into a library of your own
- Colours, styles, text styles and graphics are expandable rows of one card, like fill and
  stroke in the properties; several can be open at once (all are open at first)
- Use the “+” at the end of each section to create something new or take it from the
  selection, also by right-clicking an object; new colours and styles are chosen with the
  same colour picker as in the properties
- If the *Gradient* tab is open for fill or stroke, clicking a library colour only colours
  the selected colour stop
- Colours and styles can be named via right-click (optional); the name appears below the
  swatch
- Swatches wrap according to the width of the panel
- **Graphics:** save selected objects (e.g. a logo) as a graphic with the “+” under
  *Graphics* or the right-click menu; click or drag to copy it into any
  document – several objects arrive as a group
- Styles are shown as circles, like colours
- A click applies the colour or style to the selection; or drag it straight onto an object –
  after a short hold a preview appears
- Import, export, open folder and reload via the menu (⋮)

## Notebook view

- Instead of the canvas (all artboards side by side), pages can be shown like a notebook:
  one page at a time; anything outside it is hidden and cannot be clicked
- Scrolling moves the page; scrolling on at the end of a page turns to the next one (at the
  start, to the previous one). Page Down / Page Up turn pages as well, and in the artboards
  panel a click on a preview jumps to that page
- The floating bar at the bottom left switches between canvas and notebook (also Ctrl Shift N
  or *Preferences › View*); in the notebook it also shows “Page 2 of 5” with arrows for
  turning pages. The choice is saved and applies to all documents

## Interface

- **Language:** English (British) or German. Stijl follows the system language (German
  systems → German, everything else → English); you can choose a language under
  *Preferences › View › Language* (takes effect after a restart)
- **Report a Problem:** *Main menu › Report a Problem…* opens a new GitHub issue in your
  browser with a template and details of version and system
- **Updates (Windows and macOS):** once a day Stijl asks GitHub whether there is a new
  version and then shows a notice with *Download*. Only the version number is requested.
  Turn it off or check now under *Preferences › View › Updates*
- **Crash reports:** if Stijl crashes, a report is saved (version, system, where the error
  happened – no drawings). On the next start Stijl asks whether to send it; you can view,
  copy or discard it first
- **Start page** (Ctrl N): a short welcome, then recently opened documents (with the date
  of the last change) in one row – as many as fit – ending with the
  dashed *Open …* tile; below, the formats in the *Print* and *Online* tabs with a search
  field, ending with the dashed *Custom Format …* tile
- **Window layout:** the header bar shows the document name (with a dot when there are
  unsaved changes) and below it where the file is saved and the page format. Floating bars
  over the canvas, as in the GNOME image viewer: tools at the top left, canvas/notebook and
  pages at the bottom left, zoom at the bottom right
- Tabs for several documents. Files opened from the desktop (double-click, *Open with*)
  arrive as a new tab in the window that is already open
- With nothing selected, the properties show “Nothing selected”
- The position section (rotate, flip, align to page) is always at the bottom of the
  properties
- **Sidebars:** the *Layers* and *Library* buttons at the top left open the left sidebar
  with that content (a second click closes it; F8 shows or hides it); the *Properties* button
  at the top right (F9) shows the properties on the right, and the *information* button next
  to it shows details about the document instead: folder (with a button to open it), format,
  size, number of artboards, layers and objects, file size and dates. Both sides can be open
  together.
  Sidebars can be widened with the mouse, and the scroll wheel always
  scrolls the sidebar. In narrow windows (under about 1100 pixels) the sidebars float over
  the canvas instead, one at a time; a click next to the sidebar closes it
- **Main menu:** new, open, save and export, then Preferences, Keyboard Shortcuts (Ctrl ?),
  Help (this guide), Report a Problem and About Stijl
- **Preferences** (Ctrl ,): a sidebar with the categories **View** (colour scheme
  System/Light/Dark, background Default/Dark/Chequerboard, view Canvas/Notebook, rulers,
  unit – metric, imperial or pixels, language, updates), **Align** (“Snap to”: guides,
  corners, centres, anchor points, page, paths, spacing and grid individually or with
  *All On / All Off*; plus the delay when placing into shapes) and **Keyboard Shortcuts**; the
  magnifying glass at the top searches all preferences. *Shortcuts* lists every shortcut;
  each can be changed (with conflict checking) and reset individually or all at once
- Files are saved as SVG; layers, shadows (as filters) and gradients can be read by other
  applications too. Saving is safe: Stijl writes to a temporary file first and then renames
  it – if saving is interrupted, the old file stays intact
- **Recovery after a crash:** unsaved changes are saved to a separate recovery file every
  20 seconds (and immediately on a program error) – the original file is never touched.
  After a crash, Stijl offers to restore the documents on the next start
- **SVG files from other applications:** CSS classes (e.g. from Illustrator), linked images
  next to the file and files in ISO-8859-1 are read. If a file contains things Stijl cannot
  edit (clipping paths, masks, filters, `<use>`, patterns, nested groups …), a notice lists
  them and the file is treated as an import: saving creates a new file and the original
  stays unchanged
- **Fast effects:** shadows, watercolour, glass, 3D and texture images are cached and not
  recalculated while moving, scaling, zooming or panning – only afterwards. Full resolution
  on HiDPI screens
- **Opening Illustrator (.ai) and PDF files:** via Open, by dragging into the window or with
  *Open with*. The file is imported and treated like an SVG file: it appears as an untitled
  tab and saving asks for an SVG name – the .ai file is never changed. Requirement: saved in
  Illustrator with *Create PDF Compatible File* (the default). Imported are layers (including
  hidden ones), paths with fill and stroke, gradients (linear and radial), clipping masks,
  groups with opacity and blend mode, text (as text, with a similar installed font), images
  and all colour spaces (CMYK is converted to RGB); several pages become artboards. Not
  imported: opacity masks, pattern and mesh gradients (simplified), CMYK JPEG and JPEG 2000
  images, encrypted files

## Keyboard shortcuts

Default key bindings; they can be changed under *Preferences › Keyboard Shortcuts*.

| Shortcut | Action |
|---|---|
| V, A, R, P, N, T, G, H, K | Switch tool |
| Shift P / T / E / M | Curvature Pen / Text on Path / Eraser / Shape Builder |
| Hold Space / middle mouse button | Pan the view |
| Ctrl + scroll wheel, pinch | Zoom |
| Ctrl + / Ctrl − / Ctrl 0 / Ctrl 1 | Zoom in / Zoom out / All artboards (notebook: whole page) / 100 % |
| Ctrl Shift N | Notebook view on/off |
| Page Down / Page Up | Next / previous page (notebook) |
| Shift while dragging | Proportional, straight lines, 45° steps |
| Alt while dragging | Move: drag a copy (the original stays); shapes: from the centre; Shape Builder: delete areas |
| Enter / double-click | Finish a pen path |
| Del | Delete |
| Arrow keys (+ Shift) | Move by 1 px (10 px) |
| Ctrl C / X / V / D / A | Copy / Cut / Paste / Duplicate / Select all |
| Ctrl Z / Ctrl Shift Z | Undo / Redo |
| Ctrl G / Ctrl Shift G | Group / Ungroup |
| Ctrl ] / Ctrl [ (+ Shift) | Bring forward / Send backward (to front / to back) |
| Shift H / Shift V | Flip horizontally / vertically |
| C | Scissors |
| Ctrl J | Join paths |
| [ / ] | Smaller / larger eraser |
| Ctrl W | Close tab |
| Ctrl I | Insert image |
| Ctrl , | Preferences |
| F8 / F9 | Layers and Library / Properties |
| Ctrl N / O / S / Shift S / E | New / Open / Save / Save As / Export |
| Ctrl ? | Keyboard shortcuts |
| Ctrl Q | Quit |
