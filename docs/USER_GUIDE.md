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
- **Edit Nodes:** the properties then only show node and path options (corners, corner
  radius, Simplify Path). Click and drag nodes, select several with Shift or a selection frame; drag
  the round handle to round corners – also on rectangles and ellipses. Works on **several
  objects** at once: a frame dragged from empty space picks up the nodes of all objects
  inside it, and Shift + click on another object adds it with all its nodes (click again to
  remove it). Moving, rounding, corner/smooth and deleting apply to all selected nodes
- **Polygons and stars:** the number of corners is set in the tool options and can be
  changed later in the properties
  (position, size, rotation and corner radius are kept)
- **Corner radius:** drag the round handles in the Edit Nodes tool – one corner or several
  together – or type it under *Corner radius* in the properties (selected corners; without a
  selection all corners). The handle always stays at the same distance from the edge of the rounding and
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
  (the whole network of areas is then shown dashed), otherwise everything visible. The
  new shapes then take the place of the ones used up, so you can keep going.
  - **Click** an area: it becomes a new shape in the current fill colour (existing shapes
    stay unchanged)
  - **Drag** across several areas: they are merged into one shape (in the colour of the area
    where you started); the shapes involved fall apart into their remaining areas, open
    lines are kept
  - **Alt** while clicking or dragging: the areas are deleted from the shapes
- **Path Operations:** Unite, Subtract Front, Intersect, Exclude Overlap – under *Combine* in
  the floating bar when two or more objects are selected, and in the right-click menu. Curves
  stay curves: new anchor points only appear where the shapes cross
- **Join Paths** (right-click or Ctrl J): open paths are joined at their nearest ends – small
  gaps (under 4 screen pixels) merge into one point, larger ones are bridged with a straight
  segment. A single open path is closed
- **Offset Path …** (right-click): a new shape at a set distance around each selected shape
  (e.g. the white edge of a sticker) – positive values grow it, negative values shrink it;
  round or sharp corners. A larger shape goes underneath the original, a smaller one on top;
  open paths become a band of twice the distance
- **Simplify Path** (in the properties while editing nodes, or Ctrl L): removes unneeded anchor points from paths
  (e.g. from freehand strokes or traced images) – corners are kept, curves stay smooth

## Text

- **Edit directly in the document** (including text on a path): a click places the cursor,
  dragging or double-clicking selects, Esc finishes
- **Text options:** a collapsible card of its own (when collapsed, font and size are shown
  in its header), grouped into font, character, paragraph, text box and text on path – each
  group only appears where it applies. Text styles work as **paragraph styles**: with the
  cursor in a paragraph (or whole paragraphs selected) a style sets the font, size and
  colour of the whole paragraph and its alignment, line spacing and spacing; with only a
  few words selected, just their font. The “+” takes the style from the paragraph with the
  cursor. At the bottom are the text styles of the current
  library (like the library colours for fill and stroke); the “+” after them saves the style
  of the selected text to the library. The font is chosen from a
  searchable drop-down, each font shown in its own typeface; below it the **style** (all
  styles of the family, e.g. Light, Regular, Medium, Semi Bold, Extra Bold, Black – each also
  in italics), then size and, for **variable fonts**, weight (100–900) side by side. Underline,
  strikethrough, **small caps** and **ligatures** are icon buttons in one row under
  *Character*. *Convert to Path* (right-click) turns text into paths –
  every letter becomes a path of its own and all of them form a group (Ctrl Shift G ungroups
  it so that single letters can be moved)
- **Formatting like a word processor:** in text boxes, font, size, styles and colour (the
  fill while you type) apply only to the selected text; with a cursor and no selection, to what you type next; if the box
  itself is selected, to the whole box. If a box contains different fonts or sizes, the
  fields stay empty – a choice then applies to everything
- **Paragraphs:** left, right, centred, justified; text boxes also top/middle/bottom; line
  and letter spacing side by side; space before and after the paragraph side by side. In
  text boxes and the body text each paragraph can have its own **alignment, line spacing
  and space before/after**, like in a word processor: while you write, these settings apply
  to the paragraph with the cursor (or all paragraphs in the selection), and the sidebar
  shows that paragraph's values. With the text box selected (not writing), they apply to
  the whole text and replace the paragraphs' own settings. Enter starts a new paragraph that
  keeps the format of the one it came from, Shift+Enter only a new line. Tab inserts a tab (stops every 2.4 font sizes). Text boxes
  have an adjustable number of **columns** (1–8): text flows into the next column once the
  box height is reached. Ligatures (fi, fl …) and small caps can be switched per text
- **Line spacing:** automatic to begin with – the field shows *Auto* and each line is as
  tall as its largest text, so a bigger heading never runs into the line above. Typing a
  value or using − / + sets a fixed spacing (in % of the font size); the arrow button next
  to the field goes back to automatic
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
  *Gradient* for the fill): chequerboard, gingham, grid, dots, stripes, waves, chevron,
  herringbone, bricks, honeycomb, scales, terrazzo, zebra, leopard and cow print as previews
  (five per row), two colours of your choice, size and angle; saved in SVG as a real pattern
- **Effects:** *Add Effect* at the end of the appearance settings in the properties works on
  all selected objects at once; submenus open to the side, and choosing an effect opens its
  settings right there. Effects are added afterwards: new objects take over colours, stroke
  and font from the last ones, but no effects. One effect per category is possible, categories can be
  combined, and the strength is adjustable (except for the colour overlay):
  - **Colour overlay:** Lighten or Darken (default is Cover). Lighten mixes like spotlights:
    a single object shows its colour, consecutive Lighten objects add their light, weighted by
    opacity (red + green = yellow, red + green + blue = white). Darken mixes like layers of
    coloured glass towards black
  - **Glass:** what lies underneath appears blurred, as if seen through frosted glass.
    **Blur** sets how strongly and **Grain** adds a fine satin-glass texture. The fill
    lightly tints the glass in its colour (lower its opacity for clearer glass, or remove the
    fill for colourless glass); the stroke stays solid
  - **Material:** changes the surface – **Gloss**, **Metal** (polished, reflects a bright sky
    and a dark horizon), **Brushed metal** (with a **direction**) and **Velvet** (dark in the
    middle, shimmering at the edges). Without a material the surface is matte. Combined with
    a 3D effect, the material decides how the relief catches the light: a small sharp
    highlight for gloss, reflections that follow the shape for metal, a long streak of light
    across the brushing for brushed metal and a sheen on the steep sides for velvet
  - **Artistic effects:** grain, pixels, watercolour, **crayons** (paper shows through in
    slanted strokes), **oil paint** (brush strokes with a slight relief; on images the
    picture is also simplified into strokes), **halftone** (dots whose size follows the
    brightness; spacing and angle adjustable like a pattern) and **risograph** (colour layers
    printed slightly out of register, with grain) – on the fill of shapes and text and also
    on images. On photos they keep the picture recognisable: areas are simplified while
    edges stay sharp (watercolour adds darker rims, crayons draw the outlines, oil paint
    turns areas into brush strokes, halftone uses a four-colour print screen)
  - **3D effects:** give the object a relief with light (from the top left) and shadow; with
    transparency, what lies underneath is distorted as if seen through glass. **Depth** runs
    from −100 % (pressed in) through 0 % (flat) to +100 % (raised); **Lighting** (0–100 %,
    default 50 %) sets how bright the light and how dark the shadow are. Shapes:
    **Raindrop** (flattened sphere), **Sphere** (fully round), **Crystal** (tapering with a
    flat top), **Pyramid** (fully pointed). Without a material the relief is matte (soft light
    and shadow, no shine); not available for images
  - **Image filters** (images only): sepia, black & white, invert colours
  - **Shadow**, **Inner shadow** (the shape looks pressed in; opacity, **direction**,
    **distance** and **blur** adjustable), **Glow** – the stroke (or the shape, if it has
    none) glows in its own colour like a neon tube – and **Reflection** below the object
    (strength, **length** and **gap** adjustable): four effects of their own, each with its
    own section in the properties

  Every category applied appears in the properties as a collapsible section with its type and
  strength in the header, where it can also be removed. All sliders look the same (a round
  bar like the colour opacity) and have a field next to them for typing the number
- **Trace image:** if an image consists of flat colour areas (a logo, an illustration), the
  floating bar offers *Trace*; it opens into the number of colours and the Trace button; the image is replaced by
  grouped, editable vector areas (can be undone). Not offered for photos
- **Images:** insert (Ctrl I) or drag onto the canvas; drag onto a shape and hold briefly (until
  the ring fills) to place the image inside the shape – or inside the letters of a text. Text
  boxes, tables and charts don't take images this way; text boxes make room for them instead

## Arranging

- **Floating bar:** above (or, if there is little room, below) the selection – it holds
  quick actions; settings are in the properties. It only shows what fits the selection:
  lock/unlock and “…” for more (the same menu as the right-click menu). With several
  objects *Group* and *Combine* (the path operations), with groups *Ungroup*, with a
  clipping mask *Release Mask*, with a traceable image *Trace*. *Combine* and *Trace* open
  sideways into their options, like the tool groups; they stay open for several clicks and
  close when you click them again or select something else
- **Right-click:** cut, copy, paste, duplicate, scale, arrange, path operations, *Blend …*
  (with exactly two shapes: in-between shapes with an adjustable number of steps – shape,
  colour, stroke width and opacity are blended; both shapes and the in-between shapes are
  grouped), *Convert to Path* (text and shapes; for tables and charts *Convert to Shapes*),
  move to layer, add to library (asks: colour, style or graphic – not for tables and charts,
  which have many colours) and delete
- **Groups:** Ctrl G groups the selection, Ctrl Shift G ungroups it; a click always selects
  the whole group. Groups can contain groups: grouping groups keeps them as subgroups, and
  ungrouping removes one level at a time. Groups are saved as `<g>`, and nested groups from
  other SVG files are kept
- **Clipping mask** (right-click or Ctrl 7): the top selected object – a shape, path or text –
  cuts out the objects below it; only what lies inside it stays visible. Everything becomes
  one group, which can be moved, scaled and grouped further. *Release Clipping Mask*
  (Ctrl Alt 7) turns the mask back into an ordinary object. In SVG files the mask is saved
  as a `clipPath`, so other applications show it too
- **Selection handles:** rounded triangles in the corners, pills on the sides and a round
  rotate handle above, in the layer colour. They react a little beyond what you see, so they
  are easy to hit with a mouse or a finger. Small selections only show the corners, and the
  handles disappear while you move, resize or rotate, so you see the real edges
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
  can also be dragged out of the rulers (always shown; a click on their corner changes the
  unit). They take the colour of their layer; while you work
  with guides, the properties only show the guide settings. They snap to anchor points, corners, centres and
  edges when created and when moved later
- **Align** (*Position* in the properties): left, centre, right, top, middle, bottom. A
  single object aligns to the artboard it is on; for several objects (groups count as one)
  *Align to* chooses between *Each Other* (their common bounding box) and *Artboard* (as a
  whole). With three or more, *Distribute* spreads them horizontally or vertically – the
  outer ones stay in place, all gaps become equal
- **Commands with settings** (*Scale …*, *Repeat …*, *Offset Path …*, *Blend …*) open
  their settings on the right in place of the properties, so the object stays in view, and
  show the result straight away while you change the values. *Apply* keeps it (one undo
  step), *Cancel*, Esc or Ctrl Z go back; clicking on the canvas also applies, moving the
  view does not
- **Scale …** (right-click): exact percentages, uniform or width/height separately,
  optionally including strokes and shadows
- **Repeat …** (right-click): copies of the selection, each moved, rotated and scaled one
  step further – e.g. a row of dots, a flower of petals (rotation around the centre of the
  page) or a shrinking spiral. All copies and the original become one group
- **Copying between documents:** Ctrl C in one tab, Ctrl V in another – at the same position
- **Copy as Image** (right-click; a shortcut can be set under *Preferences › Keyboard
  Shortcuts*): the selection – or the artboard if nothing is
  selected – as an image on the clipboard, ready to paste into a chat, email or document

## Artboards and layers

- **Pages (Artboards) sidebar:** its own button at the top left, next to *Layers* and
  *Library*. Its rows look like the layer rows (preview on the left, buttons stacked on the
  right); previews in both lists grow with the width of the sidebar. Every artboard (in a PDF: every page) has a large preview like in a PDF reader;
  a click shows it (in the notebook view it turns to that page, and the current page is
  highlighted). In a document opened from a PDF, **drag a preview to change the order** of
  the pages – the order used for turning pages, printing and PDF export (numbered names
  such as *Page 3* are renumbered, your own names stay). In your own documents the order
  follows the text (notebook) or where you put the artboards (artboard view). A “+” button
  below adds one
- **Several artboards** in one file: each row has a gear button
  (size, name, **background colour** – every artboard has its own), **duplicate** (copies the artboard with everything on it and places it to the
  right) and delete – any artboard can be deleted while another one remains (if the first
  goes, the next one takes its place). While the Pages sidebar is open, artboards can be edited: dragging the name
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
- **Export:** PNG, JPG, PDF, SVG. A preview at the top shows what will be exported.
  *Export Selection…* (menu) or *Selection only* in the dialog exports only the selected
  objects, cropped to the selection. **File name:** with a running number (preset) or with
  date and time, the dialog suggests the next free name in the folder of the last export, so
  earlier exports are never replaced. With several
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
- **Print** (menu or Ctrl P): one page per artboard, at actual size and centred; artboards
  larger than the paper are scaled down to fit. Portrait or landscape follows the artboard

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
- **Global colours:** objects coloured from your own library stay linked to that colour.
  *Edit Colour …* (right-click a swatch) changes it in the library and in every linked
  object of all open documents at once (undoable in each document); documents opened later
  catch up automatically. Choosing a different colour by hand ends the link; a different
  opacity keeps it
- Swatches wrap according to the width of the panel
- **Graphics:** save selected objects (e.g. a logo) as a graphic with the “+” under
  *Graphics* or the right-click menu; click or drag to copy it into any
  document – several objects arrive as a group
- Styles are shown as circles, like colours
- A click applies the colour or style to the selection; or drag it straight onto an object –
  after a short hold a preview appears
- Import, export, open folder and reload via the menu (⋮)

## Views: artboards, notebook, presentation

Every document has one of three views. They sit as three linked buttons in the floating bar
at the bottom left; the active one is filled in the accent colour. The view is saved in the
file, so each document opens the way you left it.

- **Artboards** (like Illustrator): a free canvas with all artboards where you put them,
  for illustrations and screen graphics. New documents in pixels start here
- **Notebook** (like Word): the pages stand one below the other and scroll through, for
  writing and laying out. Whatever lies on a page moves along with it; what lies beside the
  pages is hidden and can't be clicked. Switching back to *Artboards* puts every artboard
  back where it was. New documents in print formats (mm, cm, inches, points) start here.
  Pages of different sizes are centred below each other; a new page goes to the end.
  Page Down / Page Up jump from page to page, *Whole Page* (Ctrl 0) fits the current one,
  and the bar shows “Page 2 of 5” with arrows. Ctrl Shift N switches between notebook and
  artboards
- **Body text** (notebook): the notebook is for writing. Only the Text tool is there, and
  a click anywhere on a page puts the cursor into the body text (the first click starts
  it) – no text box needed, like in a word processor. Pictures, shapes, tables and charts
  stay visible and push the text away, but are arranged in the artboard view. The text
  fills each page inside its margins and runs on to the next page; when it reaches the
  end, a new page is added, and pages added that way disappear again when the text no
  longer needs them (unless something else lies on them). ← → ↑ ↓ cross from page to
  page, Backspace at the top of a page joins it with the end of the previous one.
  Formatting works as in a text box: bold, size, colour and so on apply to the selected
  text (without a selection, to what you type next). Font, alignment, line spacing and
  columns of the whole body text are the same on every page. The properties always show
  the body text settings, open and filling the sidebar – there is no position, the page
  decides it, and no vertical alignment or padding. Below the text settings, **Page** has
  the margins (top and bottom, left and right side by side) and the first-line indent and
  columns to type in exactly, two buttons for *Insert Page Break* and *Formatting Marks*,
  and the header and footer switches.
  Lists, indents and columns keep working where objects push the text aside
- **Paragraphs:** Enter starts a new paragraph, Shift Enter a new line within the same
  paragraph. To set paragraphs apart, use the first-line indent on the ruler or
  *Paragraph spacing* in the properties; the formatting marks show ¶ where a paragraph ends
- **Header and footer** (notebook): double-click in the top or bottom margin of a page, or
  switch on *Header* / *Footer* under *Page* in the properties. The header or footer is the
  same on every page and is edited like the body text (font, size, colour, alignment);
  while you edit it, a dashed line marks it on every page. Right-click in it to insert the
  **page number**, the **page count** (“Page 3 of 12”) or the **date** – each page shows its
  own number, and the date is the day the document was last saved. While editing, these
  fields show as № (page number), ∑ (page count) and ◷ (date). *Different first page*
  leaves the first page (a title page) without them. A header or footer left empty
  disappears; switching it off removes it on all pages (Ctrl Z brings it back)
- **Page break** (notebook): Ctrl Enter, or *Insert Page Break* in the properties, starts
  the text after the cursor on the next page. Backspace at the top of that page removes
  the break again
- **Formatting marks** (Ctrl F10, or the switch in the properties): ¶ at the end of each
  paragraph, · for spaces, → for tabs, ↵ for line breaks within a paragraph, and a dashed
  line labelled *Page Break*. They are only shown in the notebook (not on the artboards or
  in the presentation), never printed or exported
- **Body Text layer:** the body text lies on its own layer at the bottom, locked. It can
  only be changed in the notebook; in the artboard view it stays on its pages and can't
  be clicked. When the body text is deleted completely, the layer goes away too
- **Page ruler** (notebook): the rulers show the current page like in a word processor –
  margins grey, the writing area white, counted from where the text begins. The handles
  are small blue tabs whose point marks the exact position on the scale. Drag the lower
  tabs on the left and right of the top ruler, or the tabs on the left ruler, to change
  the left, right, top and bottom margins; they apply to all pages (Ctrl Z takes a drag
  back). The upper tab on the left (pointing down) is the **first-line indent**: drag it
  to the right to indent the first line of every paragraph (not list items, and not a
  paragraph that continues from the previous page). Grey corner marks on each page show
  the writing area. With two or more columns, the gaps between them are grey too, with a
  small blue pill at each edge: drag the middle of a gap to move it left or right (the
  columns beside it get wider or narrower), or drag one of its edges to make the gap wider
  or narrower for all columns. The margins can also be typed in under *Text Margins* in
  the page settings (gear button in the Pages sidebar; 20 mm to begin with)
- **Presentation** (like a PDF viewer): the pages scroll like in the notebook, but nothing
  can be changed. Instead of the drawing tools there are viewing tools: **Select Text** (V –
  only text can be selected: a click puts the cursor into it, even under a picture, and
  dragging across empty space picks whole texts to copy),
  **Move View** (H), the **Highlighter** with four colours, and the **Laser Pointer** (L) – a
  glowing red trail that fades after a second. In your own documents highlights are only
  shown while presenting (Ctrl Z takes them back); they are gone when you leave the
  presentation. In a PDF they are saved as annotations
- **Full screen** (F5, or the button next to the zoom at the bottom right, only in the
  presentation): one page at a time on black. The bars appear while the mouse moves and
  disappear after two seconds. With *Select Text* or *Move View* a click, →, ↓, Page Down,
  Space or Enter go to the next page; right-click, ←, ↑, Page Up or Backspace go back;
  Home/End jump to the first/last page. The highlighter and the laser pointer draw instead.
  Esc ends the full screen

## Tables and charts

- **Data tool (D):** drag out a frame to insert a table (three rows, three columns, all cells
  looking the same – make any row stand out with fill and text settings for its cells) and type straight away; a click places one in a standard size. Click a cell
  of a table with the Data tool – or double-click it with the Select tool – to type in it.
  The floating bar offers no shape combining (union, subtract …) for tables and charts
- While typing in a cell: **Tab** / **Shift Tab** go to the next/previous cell (Tab after the
  last cell adds a row), **Enter** / **Shift Enter** go down/up, **Alt Enter** starts a new
  line in the cell, **Esc** discards the change. Each cell is one step for Undo
- While a table is selected or edited, column letters (A, B, C …) and row numbers (1, 2,
  3 …) appear around it in two rounded capsules, so you know how cells are called in
  formulas; picked columns and rows show as a blue pill in them
- **Choosing what to format:** the first click selects the whole table (dragging it then
  moves it). Once it is selected, click a cell to pick it or drag across cells to pick a
  block (Shift-click extends); click or drag across column letters or row numbers to pick
  whole columns or rows. Typing in a cell picks that cell. Fill, stroke, text colour and all
  text settings (font, size, style, weight, underline, strikethrough, small capitals,
  ligatures, alignment, line and letter spacing, text styles from the library) then apply to
  the picked cells – or to every cell when the whole table is selected. Delete clears the
  picked cells (with the whole table selected it deletes the table)
- **Fill and move like in a spreadsheet:** picked cells have a small square at their bottom
  right corner – drag it down, up, left or right to continue them: numbers count on (one
  number in steps of 1, several in their step), “Week 1” becomes “Week 2”, formulas move
  their references along (except those fixed with `$`), other text repeats; their look comes
  along. Dragging past the end of the table adds rows or columns. Drag the border of the
  picked cells to move them elsewhere in the table (formulas that refer to them follow).
  While cells are picked, the table shows no resize handles; tables have no rotate handle
- **Moving and resizing:** a selected table moves with the round grip where the column letters and
  row numbers meet. Drag the border between two column letters (or row numbers) to change
  the width of a column (or height of a row); the table grows or shrinks with it. When whole
  columns or rows are picked, all of them take the new size. Scaling the whole table with
  its handles keeps the proportions of the rows and columns
- Cells hold text, numbers, dates (`5.11.2026`, `05/11/2026`, `2026-11-05`) or formulas as
  in a spreadsheet, with + − * / ^ % & and comparisons and the functions of LibreOffice Calc
  and Excel – by their English names or the German, Spanish and French ones Calc uses
  (`=SUMME(B2:B4)`, `=SVERWEIS(…)`, `=SOMME.SI(…)`):
  - *Maths:* SUM, PRODUCT, SUMPRODUCT, SUMSQ, ABS, SQRT, POWER, MOD, ROUND, ROUNDUP,
    ROUNDDOWN, INT, TRUNC, CEILING, FLOOR, SIGN, PI, EXP, LN, LOG, LOG10, FACT
  - *Statistics:* AVERAGE, MIN, MAX, COUNT, COUNTA, COUNTBLANK, MEDIAN, STDEV, STDEVP, VAR,
    VARP, LARGE, SMALL, RANK
  - *Conditions:* SUMIF, COUNTIF, AVERAGEIF, SUMIFS, COUNTIFS, AVERAGEIFS, MAXIFS, MINIFS
    (criteria like `">5"`, `"<>red"`, `"A*"`)
  - *Logic:* IF, IFS, AND, OR, NOT, XOR, IFERROR, IFNA, ISBLANK, ISNUMBER, ISTEXT, ISERROR,
    SWITCH, CHOOSE, TRUE, FALSE
  - *Text:* CONCATENATE, CONCAT, TEXTJOIN, LEFT, RIGHT, MID, LEN, UPPER, LOWER, PROPER, TRIM,
    VALUE, REPT, SUBSTITUTE, REPLACE, FIND, SEARCH, EXACT, TEXT
  - *Lookup:* VLOOKUP, HLOOKUP, XLOOKUP, INDEX, MATCH, ROW, COLUMN, ROWS, COLUMNS
  - *Dates:* TODAY, NOW, DATE, YEAR, MONTH, DAY, WEEKDAY, DAYS

  In German, Spanish and French, numbers use a decimal comma and arguments are separated by
  `;`. While editing, a cell shows its formula; otherwise the result
- **Pointing at cells:** while writing a formula – in the cell or in the formula bar – click
  another cell of the table where a reference may go (after `=`, `(`, `;` or an operator) to
  insert it; click again to swap it, drag to make it a range (`A1:B4`). Each reference gets
  its own colour, in the formula, in the formula bar and as a frame around its cells, like in
  a spreadsheet
- **Merging cells:** pick several cells and press the merge button (next to the rows and
  columns button) – they become one cell showing the content of the top left one. Clicking a merged cell
  picks all of it; inserting rows or columns inside a merge widens it (picked, typed in or not, a merged
  cell is always framed as a whole)
- **Splitting cells:** the split button offers *Split into Columns* and *Split into Rows*. A
  merged cell is taken apart into its columns or rows; any other cell gets a new column (or
  row) beside it, half its size, and the other cells of that column (row) stretch over both
- **Formats:** *Format* sets how numbers appear in the picked cells – General, Number,
  Percent, Currency, Scientific, Date (short, long, ISO), Time (with or without seconds),
  Date and Time, or Text (shows exactly what was typed, formulas too); *Decimals* and
  *Currency* appear when they apply. Times can be typed as `14:30`, dates with a time as
  `5.11.2026 14:30`; TIME, HOUR, MINUTE and SECOND work with them. *Text Direction* (in the
  text settings) turns the text of the picked cells upwards or downwards
- **Function suggestions:** while writing a formula, the rest of a matching function name
  appears in light grey behind the cursor – in the cell and in the formula bar; Enter (or Tab
  in a cell) takes it, including the opening bracket
- **Keyboard with picked cells:** just start typing to replace the content of the picked cell
  (Enter or F2 edits it keeping the content); arrow keys move to the next cell (Shift: extend
  the block) – also while typing after starting this way; after a double-click they move the
  cursor and go to the next cell at the start or end of the text (up and down always), Tab moves right, Esc goes back to the whole table, Delete clears the cells.
  Ctrl C / Ctrl X copy or cut the picked cells as text (tabs between columns – they paste
  into other spreadsheets), Ctrl V pastes such text into the table from the picked cell on
  (the table grows if needed)
- **Properties of a table:** a formula bar with the cell name (“B3”) and its content – type
  and press Enter; one row of buttons: a menu to insert rows above or below and columns left
  or right of the picked cells (as many as are picked) or to remove their rows or columns,
  then merge and split. Below them the usual fill, stroke and text settings.
  New rows and columns take on the size and look of their neighbours. Formulas follow when
  rows or columns are inserted or removed (a reference to a removed cell becomes `#REF!`)
- **Charts from tables:** with a table selected, *Chart* in the floating bar puts a fitting chart next to it – lines for a time series (years,
  dates or rising numbers as labels), bars for several series, a pie for a few shares. The chart follows the table: change
  a number and the chart changes too. Its properties choose the type (pie, donut, bar,
  line), legend, values, where the series are (*Series In*) and – for bars and lines – the
  titles of the x and y axes. A chart keeps a copy of its data: if the table is deleted, the
  chart stays, and *Edit Data as Table* makes a new table for it
- **How the table is read:** the chart works out how the table is laid out, like a
  spreadsheet does. If the top left cell is empty, the first row and the first column are
  names; otherwise each is, when it holds text (or years). The series run along the longer
  side of the numbers: a table with years across and “M” and “F” down gives two lines, M and
  F, over the years; the same table turned gives the same chart. *Series In* (Automatic,
  Rows, Columns) overrides this. Without names, series are called “Series 1”, “Series 2” …
  and the points are numbered. Text in the top left cell becomes the title of the x axis
  unless you type your own
- **Formatting a chart:** with the whole chart selected, fill and stroke apply to its
  background and the text settings to all its text at once (labels, legend and values).
  Click it again to pick one slice, bar or point (its fill and stroke), a line or a legend
  colour (the whole series), or a single label or value (its own text settings). Left and
  right alignment arrange the legend: colour before the text, or text aligned right with the
  colour after it
- **Convert to Shapes** (right-click a table or chart): keeps only the shapes and text as an
  ordinary group – no more data, formulas or chart settings
- Tables from earlier versions keep the look of their header row – it is now the format of
  the cells of the first row and can be changed like any other
- Tables and charts are groups of ordinary shapes, so they print and export like everything
  else. Scaling one with the handles resizes the cells or the chart; the text keeps its size.
  Charts can be rotated as a whole (they stay rotated when their data changes); tables
  cannot be rotated
- **Push text away (effect):** text boxes make room for objects in front of them that push
  text away – each line uses the wider free side, or continues below the object when neither
  side has room. In the effect's settings, *Shape* chooses whether the text follows the
  object's **Outline** (a circle gets a round gap) or keeps to a **Rectangle** around it –
  grouped objects then share one rectangle around the whole group,
  and *Distance* sets the gap between object and text (12 px to begin with). While such an
  object is selected, the area it keeps free is shown as a dotted line. Objects behind the
  text don't count. Pictures, tables and charts push text away by themselves (remove the
  effect in their properties); any other shape gets it with *Add Effect › Push text away*.
  A text box can ignore all of this with *Flow Around Objects* in its properties (text from
  PDF imports doesn't flow). Lists, indents and columns keep working around objects

## Interface

- **Language:** English (British), German, Spanish or French. Stijl follows the system
  language (other languages → English); you can choose a language under
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
  over the canvas, as in the GNOME image viewer: tools at the top left, the three views and
  pages at the bottom left, zoom (and full screen in the presentation) at the bottom right
- Tabs for several documents. Files opened from the desktop (double-click, *Open with*)
  arrive as a new tab in the window that is already open
- With nothing selected, the properties show “Nothing selected”
- The position section (rotate, flip, align to page) is always at the bottom of the
  properties
- **Undo and Redo** sit at the end of the tool bar, after a divider
- **Sidebars:** the *Pages*, *Layers* and *Library* buttons at the top left open the left sidebar
  with that content (a second click closes it; F8 shows or hides it); the *Properties* button
  at the top right (F9) shows the properties on the right, and the *information* button next
  to it shows details about the document instead: folder (with a button to open it), format,
  size, number of artboards, layers and objects, file size and dates. Both sides can be open
  together.
  Sidebars can be widened with the mouse; the properties are never narrower than their
  widest setting, so they don't grow when you open a section. The scroll wheel always
  scrolls the sidebar. In narrow windows (under about 1100 pixels) the sidebars float over
  the canvas instead, one at a time; a click next to the sidebar closes it
- **Main menu:** new, open, save and export, then Preferences, Keyboard Shortcuts (Ctrl ?),
  Help (this guide), Report a Problem and About Stijl
- **Preferences** (Ctrl ,): a sidebar with the categories **View** (colour scheme
  System/Light/Dark, background Default/Dark/Chequerboard,
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
- **Opening OpenDocument texts (.odt) and templates (.ott)**, e.g. from LibreOffice Writer:
  via Open or by dragging into the window. The text becomes the body text in the notebook
  view and flows over as many pages as it needs: fonts, sizes, bold, italic, underline,
  strikethrough and colour per word; alignment, line spacing and space before/after per
  paragraph; bulleted and numbered lists with their levels; page breaks; page size,
  margins and columns; the header and footer with page number, page count and date (and
  a title page without them). Pictures and tables are placed at their paragraph and push
  the text away – check their position afterwards. Footnotes are collected at the end of the
  text. Not taken over (a note says so): comments, drawings, text frames, custom tab stops,
  different left/right pages, tables continuing over pages. The original file stays
  unchanged: saving creates a new SVG file (and a PDF can be exported)
- **PDF files open for viewing first** – in the presentation view, with the Pages
  sidebar; Page Up/Down, the arrow keys, Home and End go from page to page. Instead of the
  drawing tools there are the viewing tools and Undo/Redo:
  - **Select Text** (V): drag inside text to mark characters, or drag a frame around
    several text blocks; Ctrl C copies the text, Ctrl A marks all of it. Nothing can be moved
    or typed
  - **Move View** (H): dragging pans (Space works with every tool)
  - **Highlighter** with four colours: drag over text – nearly straight strokes become
    straight, and the stroke is as tall as the text under it. Your own highlights can be
    selected and removed with Delete until the PDF is saved. Saving writes them into the
    PDF as standard ink annotations, so every PDF reader shows them Pages can be **reordered and removed** in the
  Pages sidebar and saved back as a PDF (*Save* asks where the first time): the pages
  themselves are copied unchanged – fonts, images and text stay exactly as they were.
  Pages beyond the first 24 aren’t shown but are kept when saving
- **Switching to Notebook or Artboards** turns the PDF into a Stijl document. Stijl asks
  first, and lists fonts that aren’t installed and anything that can’t be imported. The PDF stays unchanged; saving creates
  a new SVG file. While editing, a PDF is a document of pages: one artboard per page, named
  *Page 1*, *Page 2* …, shown in the notebook view. Each page keeps its background colour (white if it has none). Lines of
  text that belong together – the same colour and size, evenly spaced, aligned left, right,
  centred or justified – become one text box, so paragraphs can be edited and re-flow;
  bold or italic words inside stay part of the paragraph. Hyphens at line ends are removed
  where a word was split, and TeX-style accents are joined with their letter. Lines only
  run on into the next one where the text really wraps; list items, short lines and table
  cells (separated by their borders) stay apart, and line spacing stays exactly as in the PDF. Headings,
  captions and labels stay separate text objects. Up to 24 pages are read

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
| F5 | Presentation full screen on/off |
| V / H / L | Presentation: Select Text / Move View / Laser Pointer |
| Page Down / Page Up | Next / previous page (notebook) |
| Ctrl Enter | Page break (notebook) |
| Double-click top / bottom margin | Edit header / footer (notebook) |
| Ctrl F10 | Formatting marks on/off |
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
| Ctrl L | Simplify path |
| Ctrl 7 / Ctrl Alt 7 | Make / release clipping mask |
| [ / ] | Smaller / larger eraser |
| Ctrl W | Close tab |
| Ctrl I | Insert image |
| Ctrl , | Preferences |
| F8 / F9 | Layers and Library / Properties |
| Ctrl N / O / S / Shift S / E | New / Open / Save / Save As / Export |
| Ctrl P | Print |
| Ctrl ? | Keyboard shortcuts |
| Ctrl Q | Quit |
