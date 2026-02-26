# Loco Positioning System

The `loco` command displays anchor information from the Loco Positioning System
(LPS) v2 memory on the Crazyflie. This requires an LPS deck to be attached.

## Display anchor data

To display the configured anchors, their positions, and active status:

```bash
cfcli loco display
```

This will show output similar to:

```text
Loco Positioning System - Anchor Data:
   ID  Active  Valid  Position (x, y, z)
    0     yes    yes  (1.000, 2.000, 0.500)
    1     yes    yes  (4.000, 2.000, 0.500)
    2     yes    yes  (1.000, 5.000, 0.500)
    3     yes    yes  (4.000, 5.000, 0.500)
    4      no    yes  (0.000, 0.000, 3.000)
    5      no    yes  (4.000, 0.000, 3.000)
```

The columns show:

- **ID** - The anchor identifier
- **Active** - Whether the anchor is currently being used for positioning
- **Valid** - Whether the anchor has valid position data stored
- **Position** - The 3D coordinates (x, y, z) of the anchor in meters
