# Field-to-finish: linework and symbols — design

Status: **phases 1 and 2 built.** Phase 1 (`landsurvey::finish`, `LS_FINISH lines`):
string-numbered and XYZ coding, ordering, closure, `Y` spurs, `*` joins, re-run
safety. Phase 2 (`landsurvey::library`, `LS_LIBRARY`): AutoMAP CSV libraries with
connect, line layer, colour, linetype, 2D/3D and width, AutoMAP lookup and
wildcards. Later phases are still design.

Notes from building phase 2:

- AutoMAP treats `COLOR` and `LINE TYPE` as the properties of the line layer
  it creates. Phase 2 sets them on each figure instead, and the host registers
  the layer with default properties when a figure first uses it. Creating the
  layers properly is a host-scripting follow-up; see "Host scripting" below. A linetype the drawing has not loaded is left
  by layer and reported, because the DWG writer can only reference loaded
  linetypes.
- `COLOR` 0 (by block) and 256 (by layer) both mean "by layer"; 1–255 are
  colour indexes. MicroSurvey's own libraries use all three.
- `LINES` (individual lines vs polyline) is read but not used; figures are
  always polylines.
- MicroSurvey ships linework libraries to test against (`default.csv`,
  `feature_demo.csv`, `leica-code-list.csv`, `nikon.csv`, `survey.csv` in
  `ProgramData\MicroSurvey\MSCAD\2027\mscad`); all of them parse, and the
  Lab 8 strings connect only where a library has the code (`BLDG1` via
  `BLDG` in `default.csv`, `GL1` exactly in `nikon.csv`).

## Goal

The surveyor codes points in the field. The plugin turns those codes into a
finished drawing:

- **linework**: edges of road, buildings, fences, curbs and ditches, drawn as
  polylines on the right layers, with straight and curved segments;
- **symbols**: monuments, iron pins, posts, utility features, placed at their
  points at the right scale, on the right layer.

It works the same way after `LS_RAW` (RW5 / SDR), `LS_PNEZD` or
`LS_LANDXML`, and it can be re-run after the codes are edited.

This is the most-requested capability that the plugin still lacks. It is the
core of MicroSurvey AutoMAP, Carlson field-to-finish and Civil 3D figures and
point styles. The plugin already layers points by code (`LS-PT-<CODE>`), but it
does not yet connect them or give them symbols.

Both halves are driven by **one code library**, one row per code, as in
AutoMAP: a code can connect, carry a symbol, or both.

## What the coding looks like in practice

These are the codes in the Lab 8 FieldGenius job (`TopoSurvey2023.raw`, 64
points):

| description | count | meaning | result |
|---|---|---|---|
| `ER1` … `ER2` | 6, 2 | edge of road, strings 1 and 2 | line |
| `GL1` | 3 | gutter line, string 1 | line |
| `RWT1` | 4 | retaining wall top | line |
| `BLDR1`, `BLDR2 ROOFPK` | 4, 2 | boulders 1 and 2 (`ROOFPK` is a note) | line |
| `VLTC1` | 3 | vault corner | line |
| `BLDG1 STUCCO` | 2 | building 1, with a note | line |
| `OIP LOT 28` | — | found iron pin | **symbol** (`ICM_FSIP`) |
| `OCM` | — | found concrete monument | **symbol** (`ICM_CM`) |
| `G`, `TH CN`, `CHK 27` | — | ground shot, control, check | point only |

The user's AutoMAP library, `MSCAD_2031.CSV` (38 rows), already maps `OIP`,
`OCM`, `IP`, `ODIP`, `OAI`, the other `ICM_*` monuments, `RT1M`–`RT18M` and
`MY_POST` to blocks. So the symbol half works on real data with the library
the user already has. The linework half needs a library with `CONNECT = 1`,
or the no-library default described below.

## BC standards this follows

Three published BC sources shape the symbols, the codes and the output:

1. **Plan symbols: General Survey Instruction Regulation, B.C. Reg.
   229/2001, s. 29(1)**
   ([BC Laws](https://www.bclaws.gov.bc.ca/civix/document/id/loo62/loo62/229_2001)).
   - These standard symbols "must be used on all survey plans", each with a
     **found** and a **placed** form and a size in plotted millimetres (see
     3).
   - Section 27(1)(d) requires a legend of the monuments shown, and only
     those.
   - The BC Laws page is an archived consolidation (Queen's Printer,
     2003–2004). These symbols are long-standing and very stable, so they
     are the starting point; they can be adjusted if a newer rule set
     changes any of them.
2. **LTSA survey plan dataset**
   ([LTSA help](https://help.ltsa.ca/ltsa-enterprise/tips-and-tricks-survey-plan-dataset-creation)).
   A plan is submitted with:
   - a `.dwg` on nine fixed `PMBC_*` layers;
   - a 10-field, header-less control-point `.csv`.

   Several of its rules constrain field-to-finish output:
   - all entities 2D with Z = 0;
   - no Civil 3D objects;
   - circles only on `PMBC_OTHER`;
   - polylines only for natural boundaries and survey limits; two-point
     lines and arcs otherwise;
   - **"if a line was clipped around a post symbol, extend it to its true
     intersection"**.

   So symbol trimming must never alter the stored geometry (see 3), and
   symbols must stay off the `PMBC_*` boundary layers. A `PMBC` export and
   checker is a natural follow-on; it gets its own design (see 6).
3. **Feature codes: BC Ministry of Transportation and Transit (MoTI) General
   Survey Guidelines, Appendix E**, *Survey Codes and Attributes 1.3*
   (2019-06-26). This is a published provincial code list of about 330
   features. Each row has:
   - a description;
   - a two-letter **code**, e.g. `EP` pavement edge, `FE` fence, `IP` iron
     pin – standard, `CP` iron pin – capped, `LM` control monument –
     geodetic, `MN` monument – concrete post, `DH` hub – detail / traverse
     hub, `TE` tree;
   - a **DTM** flag (Y/N);
   - up to five **attribute** names, e.g. `Condition`, `Diameter`, `Type`,
     `Notes 1`.

   Appendix E does not say which codes are lines and which are points, nor
   how strings are numbered. That is presumably in the body of the
   guidelines, which I have not seen.

**Consequences for this design**

- The DTM flag maps directly onto AutoMAP's `EXCLUDE FROM DTM` (N →
  exclude).
- MoTI codes **collide with MicroSurvey's XYZ prefixes**:
  - `XG`, `XO`, `XR` … begin with `X`;
  - `YL` and `YP` begin with `Y`;
  - `ZA` … `ZW`, `ZL` and `ZZ` begin with `Z`.

  Read as prefix coding, `ZL` would become "line, code `L`". So the
  tokeniser must try the whole first word against the library *before*
  stripping a prefix, and should warn when a loaded library makes `xyz` mode
  ambiguous.
- They also collide with the proposed control tokens (`PC` pipeline –
  plastic, `PT` power/telephone pole) and with course usage (`TH` is "test
  hole" in MoTI but "traverse hub" in the Lab 8 data). So:
  - control tokens are only recognised **after** the code, never as the code
    itself;
  - the loaded library, not a built-in guess, decides what a code means.

### Line coding

The Lab 8 data uses the **string-numbered** convention. The first word is a
feature code plus a string number. Every point with the same first word joins
into one line, in the order it was shot. Anything after the first space is a
note. Civil 3D figure prefixes and MicroSurvey's AutoMAP "Connect" option
both work this way.

MicroSurvey also documents a **prefix (XYZ) convention**, from its help
topic *Connect Points by XYZ Coding*:

| prefix | meaning |
|---|---|
| `Z` | line: joins the previous point with the same base code; the first point *without* a prefix ends the string |
| `X` | point on a curve |
| `Y` | sideshot spur from the last `Z`/`X` point, without breaking the string |
| `.` | close the figure back to its first point, and end it |
| `A*B` or `A.B` | the point belongs to two strings (e.g. `ZCURB1*ZSIDEW1` at an intersection) |
| text after a space | ignored |

The help gives worked examples with exact expected connections, for instance
`ZSIB, ZSIB, ZCURB, ZSIB, ZCURB, SIB, ZCURB, ZCURB, CURB` → 1-2-4-6 and
3-5-7-8-9. Those examples become unit tests directly.

## Design

### 1. Engine: `landsurvey::finish` (host-free, `std` + `serde`)

```text
finish::build(points: &[CodedPoint], library: &Library, opts: &Options) -> Finish
CodedPoint { name, n, e, z, description }      // in shot / file order
Finish     { figures: Vec<Figure>, symbols: Vec<Placement>, warnings }
Figure     { code, string, layer, style, closed: bool,
             vertices: Vec<Vertex { point: usize, curve: bool }> }
Placement  { point: usize, code, symbol: SymbolRef, scale, rotation, layer }
```

**Linework**

1. **Tokenise each description.** Split off the first word and remove any
   coding prefix (`Z`, `X`, `Y`, `.`), following the library's coding
   mode. Read control tokens such as `B`, `E`, `CLS`, `PC` and `PT`
   (configurable; see 4). Expand `*` joins into one membership per string.
2. **Assign each point to strings.** The string key is the code plus its
   number (`ER1`). Only codes the library marks as connecting form strings.
   With no library, any code ending in a digit forms a string.
3. **Order each string's points** by shot (file) order, which is the default,
   or by point number, as MicroSurvey's XYZ help specifies. This is an
   option.
4. **Turn curves into geometry.** A run of curve points (`X…`, or a
   `PC…PT` span) between two line points is replaced by circular arcs, each
   arc through three consecutive points, and stored as bulges. With fewer
   than three points on a curve, the run is drawn as straight segments and a
   warning is given.
5. **Closure.** A figure closes on the `.` prefix or the `CLS` token, or
   automatically when its last point repeats its first.
6. **Trim at symbols (drafting only).** Where a figure vertex carries a
   symbol with a trim radius (`SYM TRIM`), the *drawn* segments touching it
   can stop at that radius, so a fence line does not run through a monument
   symbol. The figure keeps its untrimmed vertices in its
   `LANDSURVEY_FIGURE` record, because the LTSA dataset rules require lines
   to run to the true corner. Any export uses the untrimmed geometry.

**Symbols**

7. **Match each point's base code** to a library row, by exact key first and
   then by wildcard (`PIN*`, `*PIN*`). The base code is the first word, with
   any coding prefix and string number removed, so `OIP LOT 28` matches
   `OIP` and, if the row has a symbol, `ZFENCE3` can be both a line and a
   post symbol.
8. **Place one symbol per matching point** at the point's N/E (and Z for 3D),
   with the row's scale and layer. Rotation is 0 for now; the AutoMAP
   rotation columns (`ROTATE LAST`, `ROTATE RANDOM`, `NO SYM ROTATION`) come
   later.

**Warnings** cover:

- strings with only one point
- codes that are not in the library
- a curve that cannot be fitted (three collinear points)
- a `Y` spur with no string to attach to
- a symbol whose drawing cannot be found or read

The engine is pure and deterministic, so every rule can be unit-tested
without the host. It never reads files itself: symbol geometry is resolved by
the plugin (see 3) and handed in.

### 2. Library: MicroSurvey's AutoMAP CSV, read as-is

Users who already have an AutoMAP library keep it. The engine reads the
MicroSurvey CSV columns that matter, by header name (the file starts with a
UTF-8 byte-order mark, which is stripped):

| AutoMAP column | use |
|---|---|
| `KEY` | the code; `*` wildcards (`PIN*`, `*PIN*`) as in AutoMAP's extended options |
| `CONNECT` | 1 = this code forms strings |
| `LINE LAYER`, `LINE TYPE`, `COLOR` | figure layer and style |
| `2D` | 1 = 2D polyline (`LwPolyline`), 0 = 3D (`Polyline3D`) |
| `LINES` | lines vs polyline (the plugin always draws polylines; recorded only) |
| `SYMBOL` | symbol drawing, e.g. `BLOCKS\ICM_FSIP.DWG`, relative to the symbol folder |
| `SYM SCALE`, `SYM SCALE TOG` | symbol scale, and whether the drawing's plot scale also applies (to confirm against the AutoMAP help) |
| `SYM LAYER` | symbol layer (`BLOCK` in the user's library) |
| `SYM TRIM` | radius at which connected lines stop short of the symbol (to confirm) |
| `DESC LAYER`, `DESC TEXT` | point layer / plotted description (phase 4) |
| `EXCLUDE FROM DTM` | phase 5: keep these points out of `LS_SURFACE` |
| rotation, `MARKER *`, label offsets | later |

The library loads with `LS_LIBRARY LOAD <file.csv> [symbols=<folder>]` and
lasts for the session, like `LS_LABELSTYLE`. Without a library:

- numbered codes connect on `LS-LN-<CODE>` layers in a default style;
- the built-in s. 29(1) monument symbols apply to their codes (see 3).

**A MoTI preset.** `LS_LIBRARY LOAD moti` would load the MoTI Appendix E
codes:

- code and description;
- the DTM flag as `EXCLUDE FROM DTM`;
- the attribute names, kept for a later attribute-capture phase;
- built-in symbols for the monument codes (`IP`, `CP`, `MN`, `LM`, `RM`,
  `LP`, `AI`, `AP`, `DP`, `IO`, `IS`, `DH`, `PM`).

Which MoTI codes connect into lines is not in Appendix E (see "BC standards"
above), so the preset leaves that unset until it is confirmed.

**Standards presets.** MoTI is the first of what should become a set of
presets, one data file per jurisdiction or agency under
`crates/landsurvey/presets/`. Each file is in the same CSV format as a user
library, with a header comment citing its source and version. Each new file
is added on request, so a user who wants their province, state or DOT code
list can contribute one without touching code. `LS_LIBRARY LOAD <preset>`
and `LS_LIBRARY SHOW` list what is available.

The user's library has `CONNECT = 0` on every row, so it exercises symbols
only. A course library with linework would be good to test against.

### 3. Symbols

**Where symbol geometry comes from**

The built-in set (below) is the primary source. Reading existing symbol
libraries is a nice-to-have and comes later (see 6).

- **Existing symbol drawings (later).** A library `SYMBOL` path such as
  `BLOCKS\ICM_FSIP.DWG` is resolved against a symbol folder given on
  `LS_LIBRARY LOAD … symbols=<folder>`, by default the CSV's own folder. For
  a MicroSurvey install that is `C:\ProgramData\MicroSurvey\MSCAD\<year>\mscad`,
  which holds `blocks\ICM_*.dwg`. `Symbols\` beside it holds more: APWA
  utilities, buildings, landscape, posts and road markings (1,266 drawings
  in the 2027 install). The plugin reads each `.dwg` with the codec's DWG
  reader (`codec::io::dwg::DwgReader`, already in the plugin's pinned
  `opencadcodec` build), once per session, and caches its model-space
  geometry.
- **A built-in set: the B.C. Reg. 229/2001 s. 29(1) standard symbols,**
  drawn for this project from the regulation's table, so the plugin works
  without MicroSurvey. They are defined in the plugin source as simple
  geometry (circles, squares, triangles, solid fills). Plain `.dxf` symbol
  files are also accepted, so users can draw their own.

**The s. 29(1) symbols**

The regulation gives each symbol's shape as an image and one or two
dimensions in plotted mm. Reading two dimensions as **outer, inner** is my
interpretation of the images, to be checked. "Found" fills the inner shape.
The MicroSurvey block names are the ones in `mscad\blocks` that appear to
correspond (`ICM_F…` found, `ICM_P…` placed).

| symbol | shape (placed / found) | mm | MicroSurvey block |
|---|---|---|---|
| Standard concrete post (Type 1) | circle with inner circle; inner filled when found | 3.5, 1.5 | `ICM_PSCP` / `ICM_FSCP` |
| Standard rock post (Type 2) | circle with inner square; filled when found | 3.5, 2.0 | `ICM_PSRP` / `ICM_FSRP` |
| Lead plug (Type 3) | square; filled when found | 2.5 | `ICM_PLP` / `ICM_FLP` |
| Standard capped post (Type 4) | circle with ×; top and bottom quadrants filled when found | 3.0 | `ICM_PSC` / `ICM_FSC` |
| Standard iron post (Type 5) | circle; filled when found | 2.5 | `ICM_PSIP` / `ICM_FSIP` |
| Standard wooden post (Type 6) | square with ×; top and bottom triangles filled when found | 2.5 | `ICM_PSWP` / `ICM_FSWP` |
| Control monument | circle with filled triangle (found only) | 3.5, 2.5 | `ICM_CM` |
| Old pattern Dominion iron post | square with filled inner square (found only) | 3.0, 1.5 | `ICM_DIP` |
| Non-standard square iron post | square with filled inner diamond (found only) | 3.0, 2.5 | — |
| Non-standard round iron post | square with filled inner circle (found only) | 3.0, 2.0 | — |
| Angle iron | triangle with small filled triangle (found only) | 3.5, 1.0 | `ICM_AI` |
| Aluminum post | filled diamond (found only) | 2.0, 2.5 | `ICM_AP` |
| Traverse hub | triangle; filled when found | 2.5 | `ICM_PTH` / `ICM_FTH` |
| Unmarked measured point | + (placed only) | 1.0 | `ICM_UMP` |

**Sizes are plotted millimetres.** The size in the drawing is mm × plot
scale ÷ 1000; for example, a 2.5 mm iron post at 1:500 is 1.25 m across. So
symbol scale follows the drawing's plot scale, which is presumably what
AutoMAP's `SYM SCALE TOG` switches. The plot scale comes from the existing
label settings.

**Found or placed** is part of the code. The user's library already
distinguishes the two: `OIP` (found iron post) → `ICM_FSIP`, `IP` →
`ICM_PSIP`. The built-in library uses the same `O` prefix for "found",
following that file. It is configurable, because MoTI's `IP` / `CP` / `MN`
codes carry found or placed in the `Condition` attribute instead.

**Legend.** Section 27(1)(d) requires a legend of the monuments on the plan.
Since `LS_FINISH` knows which symbols it placed, a later `LS_LEGEND` can
draw exactly that list.
- **MicroSurvey's drawings are never copied into the repo or a release.**
  They are MicroSurvey's content; the plugin only reads them where the user
  has them installed. Tests that use them are opt-in, like `RW5_SAMPLE`.

**How symbols are drawn**

The host's plugin interface (API 7) can add, update and remove entities, but
cannot create **block definitions**: an out-of-process plugin has no call for
it, and edits through `document_mut()` are not sent back to the host. So
there are two ways to draw a symbol:

| | how | pros | cons |
|---|---|---|---|
| **A. Plain geometry (phase 3)** | Transform the symbol's lines, arcs, circles and polylines to the point (scale, rotate, move) and add each as an entity, tagged `LANDSURVEY_SYMBOL [point, code, symbol]` | works on the current host, no host change | not a real block: cannot be redefined in one place; larger drawings; harder to select as one object |
| **B. Real blocks** | Define each symbol once as a block, then add one `Insert` per point | how MicroSurvey and Civil 3D do it; small drawings; redefine in one place | **no host change needed after all:** API v7 (v2026.41, the pinned host) has `TableOperation::BlockCreate`, which defines a block from drawing entities; see "Host scripting" below |

**Proposal:** ship A first, and propose B to OpenCADStudio in parallel. The
`LANDSURVEY_SYMBOL` tag lets a later version replace A's geometry with
inserts in one pass.

Symbol drawings may contain things that plain geometry cannot carry over:
wipeouts (the `ICM_*_WIPEOUT` variants), attributes, hatches and nested
blocks. Phase 3 draws lines, arcs, circles, polylines and solid fills,
explodes nested blocks, and reports anything it skips.

### 4. Coding modes and control tokens

A library or command option selects the line coding mode:

- **`strings`** (default): string-numbered codes as above. Optional control
  tokens follow the code, Civil 3D / Carlson style:
  - `B` begin a new string
  - `E` end the string
  - `CLS` close the figure
  - `PC` … `PT` curve between those points

  The token words are configurable, because firms differ.
- **`xyz`**: MicroSurvey prefix coding (`Z` / `X` / `Y` / `.`, with `*`
  joins).
- **Never both at once.** MicroSurvey's own help warns that running AutoMAP
  Connect together with Z-coding draws every line twice.

Symbols do not depend on the coding mode: they always match the base code.

### 5. Plugin commands

- **`LS_FINISH [lines] [symbols] [preview] [order=shot|number] [mode=strings|xyz]`.**
  Builds linework and places symbols from the Land Survey points already in
  the drawing, read from their `LANDSURVEY_POINT` records in import order, so
  it works after any importer. With neither `lines` nor `symbols`, it does
  both.
  - Figures are drawn as polylines and tagged `LANDSURVEY_FIGURE [code,
    string, first point, last point, count]`; symbol geometry is tagged
    `LANDSURVEY_SYMBOL` (see 3).
  - Re-running first removes what it tagged last time
    (`HostApi::remove_entity`), so editing a description and running it
    again is safe and never doubles anything.
  - It prints a report: figures per code (open or closed), symbols per code,
    and the warnings.
- **`LS_LIBRARY LOAD <file.csv> [symbols=<folder>] | RESET | SHOW`.** Manages
  the code library. `SHOW` lists each code with its line style and symbol,
  and flags symbol drawings that cannot be found.
- **`LS_AUTOFINISH ON|OFF`.** When ON, `LS_RAW`, `LS_PNEZD` and `LS_LANDXML`
  run `LS_FINISH` after importing. It is OFF by default at first.
- **LandXML export.** `LS_LANDXMLOUT` writes figures as `<PlanFeatures>`, which
  the exporter already supports, so linework reaches Civil 3D and
  MicroSurvey. Symbols are not exported; the receiving program places its
  own from the point codes.

### Host scripting (to do)

Some of what AutoMAP does to a drawing is table work, not entity work:
creating layers with a colour and linetype, loading linetypes, defining
blocks. Phase 2 assumed an out-of-process plugin could not do it. That is out
of date. The pinned host (v2026.41, plugin API v7) can be scripted from the
plugin, over the same IPC connection as `add_entity`:

| need | host call (API v7) | use |
|---|---|---|
| layer with properties | `HostApi::add_layer(LayerConfig { name, color, linetype, lineweight, … })`, or `table_operation(TableOperation::LayerCreate { config })` | create each AutoMAP `LINE LAYER` (and later `SYM LAYER`, `DESC LAYER`) with the row's `COLOR` and `LINE TYPE`, as MicroSurvey does, instead of styling every figure |
| change an existing layer | `modify_layer` / `TableOperation::LayerModify` | leave existing layers alone by default; offer an option to update them |
| define a block | `TableOperation::BlockCreate` (from entities, with `erase_originals`) | real symbol blocks for phase 3 (option B above): draw a symbol once, define it, insert it per point |
| anything else | `execute_command(cmd)` (command line, AutoLISP / script style) and `run_command(CommandRequest)` | e.g. loading a missing linetype from a `.lin` file, if the host has a command for it; to be checked |

To do:

1. Before drawing, create any missing figure layer with `add_layer`, taking
   colour and linetype from the library row; set colour and linetype on the
   figure only when the layer already existed with different properties.
2. Find out whether the host can load a linetype by name (a `LINETYPE` load
   command through `execute_command`), so `CENTER`, `HIDDEN` and the like
   work in a blank drawing.
3. Use `BlockCreate` for symbols in phase 3 (option B), not exploded
   geometry.
4. The test host returns "not supported" for all of these (the trait
   defaults), so the glue must fall back to today's behaviour, and the
   field-test suite (real host) is where the scripted path gets checked.

### 6. Phases

| phase | scope | size |
|---|---|---|
| 1 | engine: string-numbered and XYZ modes, ordering, closure, `Y` spurs, `*` joins; `LS_FINISH lines` (straight segments) with re-run safety; no library needed | M |
| 2 | AutoMAP CSV library: connect, line layers, line types, colours, 2D/3D, wildcards; `LS_LIBRARY` | S–M |
| 3 | symbols as plain geometry: the built-in s. 29(1) set, found and placed, scaled to plot scale, on the library's layer; `.dxf` user symbols; `SYM TRIM` drafting trim; `LS_FINISH symbols`; MoTI preset | M |
| 4 | curves (`X` runs, `PC`/`PT`) as 3-point arcs → bulges; 3D polylines; plotted description / description layer | S–M |
| 5 | figures as surface breaklines; `EXCLUDE FROM DTM`; `LS_AUTOFINISH` | M |
| later | reading existing DWG symbol libraries (MicroSurvey `ICM_*` etc.); real blocks (needs the host change); symbol rotation; `LS_LEGEND`; MoTI attributes; FieldGenius figure files (`_figures.dbf`, which MicroSurvey's `PROC_FIGURE` reads) | M |
| separate design | **LTSA plan dataset**: export to the nine `PMBC_*` layers plus the 10-field control `.csv`, and check the published rules (Z = 0, closed parcel polygons, no overshoots or gaps, lines broken at intersections, circles only on `PMBC_OTHER`, single-line `PMBC_DIM` text) | M–L |

Symbols come before curves because the user's data and library exercise
symbols today: `OIP` and `OCM` are in the Lab 8 job, and the library already
maps them.

### 7. How it will be verified

- **MicroSurvey's documented examples** become unit tests with the help
  topic's exact expected connections: the Z sequence, X curve, Y sideshot
  and `.` close examples.
- **Lab 8 real data.** The string codes in `TopoSurvey2023.raw` are checked
  against Civil 3D's figures for the same survey. The Lab 8 survey database
  has `Figure` and `FigureNode` tables. They are student-built, so they are
  used only as a cross-check of which points connect, never as coordinate
  goldens.
- **Symbols.** Unit tests place the built-in symbols and check the
  transformed geometry exactly. An opt-in test reads the installed
  `ICM_*.dwg` files, places them on the Lab 8 monuments, and checks each
  symbol's extents against the drawing's own.
- **A MicroSurvey capture (optional, if the licence allows).** Load the same
  raw file with the user's library (plus `CONNECT = 1` for the string codes),
  run *Process All Connections*, and compare the polylines and symbol
  positions.
- **Opt-in tests** follow the existing `RW5_SAMPLE` / `SDR_SAMPLE`
  pattern; real field files and MicroSurvey's drawings are never committed.

## Decisions needed

1. **Default coding mode.** I propose `strings`, which matches the course
   data and Civil 3D, with `xyz` selectable. Does your firm or course use
   XYZ prefixes?
2. **Default point order.** I propose shot (file) order, which is what
   string coding expects; MicroSurvey's XYZ help says increasing point
   number. Should `xyz` mode default to point-number order?
3. **Control-token words.** I propose `B`, `E`, `CLS`, `PC`, `PT`. Do you
   have a code sheet from the course (the Civil 3D linework code set or
   figure prefix database) to match instead?
4. **2D or 3D by default** when there is no library. I propose 2D
   `LwPolyline`, which is plan drafting, with 3D available per library row.
5. **Symbols as plain geometry first,** with real blocks once the host can
   define them. Should I also open the host-side proposal (a "define block"
   call in the plugin API) on OpenCADStudio?
6. **MoTI line rules.** Which MoTI codes are lines, and how their strings are
   numbered and begun, should be in the body of the MoTI General Survey
   Guidelines. Until that is found, the MoTI preset carries codes, DTM flags
   and monument symbols only.
7. **Command name.** `LS_FINISH` covers both halves; `LS_LINEWORK` and
   `LS_SYMBOLS` could be added as shorthands.

**Decided** (2026-10-12):

- Built-in symbols are the full s. 29(1) set, found and placed. It is a good
  starting point; adjust later if needed.
- Ship a MoTI preset (codes, descriptions, DTM flags, with a citation, not
  the PDF), as the first of a set of per-jurisdiction presets.
- Build order as in section 6: phases 1 and 2 first, then symbols.
- Reading existing symbol libraries (MicroSurvey DWGs) is a nice-to-have,
  moved to "later".
