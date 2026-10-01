# Revit (Windows)

The ribbon, Properties palette, Project Browser, Type Selector and dialogs
are in the tree; the drawing area is not. Exact numbers go in through
typed lengths, temporary dimensions and the Properties palette.

## Before you start

- Start a new project from the template the user uses (File ▸ New ▸
  Project). The architectural template is the usual default.
- Units: Manage ▸ Project Units (`UN`). Check length units and rounding.
- Levels: open an elevation in the Project Browser and set each level's
  height in its Properties or by its elevation value.
- Work in a plan view: Project Browser ▸ Floor Plans ▸ Level 1
  (double-click).

## Two-letter shortcuts

Press the letters with no modifier. `Escape` twice ends any command.

| Keys | Command |
|---|---|
| `WA` | Wall |
| `DR` | Door |
| `WN` | Window |
| `CM` | Component |
| `RM` | Room |
| `DI` | Aligned dimension |
| `GR` | Grid |
| `LL` | Level |
| `RP` | Reference plane |
| `MV` | Move |
| `CO` | Copy |
| `RO` | Rotate |
| `MM` | Mirror (pick axis) |
| `AL` | Align |
| `TR` | Trim/Extend to corner |
| `OF` | Offset |
| `VV` | Visibility/Graphics |
| `ZF` | Zoom to fit |

## Walls with exact lengths

1. `WA`. In the Type Selector (Properties palette, top) choose the wall
   type.
2. In the Options Bar set Height (or Unconnected and a value), Location
   Line and Chain on.
3. Click the start point (a grid intersection or a known corner; use a
   `screenshot(app, grid=...)` to find it).
4. Type the length with `type_text("5000\n")`, giving that same call an
   `x`/`y` a little way along the wall's direction: the pointer sets the
   direction. Repeat for each segment.
5. A rectangle of walls: in the Draw panel choose Rectangle, click two
   opposite corners, then fix the sizes with temporary dimensions (below).
6. `Escape` twice to finish.

## Exact positions afterwards

- Select an element. Temporary dimensions appear. Click the dimension's
  number, type the value, `Return`: the element moves to match.
- Move by a distance:
  1. `MV`.
  2. Click a start point.
  3. `type_text` the distance with `\n`, with `x`/`y` a little way in the
     direction to move.
- Properties palette: offsets, heights and constraints are fields. Click,
  type, then Apply or `Return`.

## Doors, windows, floors, rooms

- Doors and windows:
  1. `DR` / `WN`. Pick the type in the Type Selector.
  2. Click on the wall roughly where it goes.
  3. Set its exact offset with the temporary dimension to the nearest
     wall end or grid.
- Floor:
  1. Architecture ▸ Floor.
  2. Pick Walls.
  3. Click each bounding wall (or Tab-select the chain).
  4. Finish Edit Mode (the green check on the ribbon).
- Rooms: `RM`, click inside each enclosed space; tags come with them.
  Name them in Properties.

## Views, sheets, export

- 3D: the Default 3D View button, or Project Browser ▸ 3D Views ▸ {3D}.
- Sheets: View ▸ Sheet, then drag views from the Project Browser onto it.
- Export:
  - File ▸ Export ▸ PDF (newer versions), CAD Formats (DWG) or IFC.
  - Save `cmd+s` (`.rvt`).

## Notes

- A warning dialog (yellow) can be read and closed; an error dialog
  (red) needs a decision. Read it, and undo if unsure.
- Dynamo and pyRevit scripts are exact but run code: only with the user's
  OK (security rule 2).
