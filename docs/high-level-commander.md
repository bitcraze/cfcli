# High-Level Commander

The high-level commander provides autonomous flight capabilities for the Crazyflie. It handles
takeoff, landing, position control, and trajectory execution. All commands require the Crazyflie
to have a valid position estimate (from a positioning system like Lighthouse or Loco).

```text
Usage: cfcli hlc <COMMAND>

Commands:
  arm         Arm the Crazyflie (enable motors)
  disarm      Disarm the Crazyflie (disable motors)
  takeoff     Take off to a specified height
  land        Land at the current position
  goto        Go to a specified position
  stop        Stop all high-level commands and disable motors
  trajectory  Trajectory operations
  help        Print this message or the help of the given subcommand(s)
```

## Arm and Disarm

Before flying, the Crazyflie must be armed. This enables the motors and allows the high-level
commander to control the drone.

```text
cfcli hlc arm
cfcli hlc disarm
```

## Takeoff

Take off to a specified height. The duration specifies how long it should take to reach the target height.

```text
cfcli hlc takeoff [-z <HEIGHT>] [-d <DURATION>] [-y <YAW>]
```

Defaults: `--height 0.5`, `--duration 2.0`.

### Takeoff Examples

```text
# Take off to 0.5 meters over 2 seconds (defaults)
cfcli hlc takeoff

# Take off to 1 meter over 3 seconds
cfcli hlc takeoff -z 1.0 -d 3.0

# Take off to 0.5 meters while rotating to 90 degrees yaw
cfcli hlc takeoff --yaw 90
```

## Land

Land at the current position. The height parameter specifies the target landing height (typically 0.0).

```text
cfcli hlc land [-z <HEIGHT>] [-d <DURATION>] [-y <YAW>]
```

Defaults: `--height 0.0`, `--duration 2.0`.

### Land Examples

```text
# Land over 2 seconds (defaults)
cfcli hlc land

# Land over 3 seconds
cfcli hlc land -d 3.0

# Land while rotating to 0 degrees yaw
cfcli hlc land --yaw 0
```

## Go To Position

Move to a specified position. The position is given as comma-separated x,y,z coordinates.

```text
cfcli hlc goto <POSITION> [-d <DURATION>] [-y <YAW>] [-r]
```

The position format is `x,y,z` where `x`, `y`, `z` are coordinates in meters.

Options:

- `-d, --duration`: Time in seconds to reach the position (default: 2.0)
- `--yaw`: Target yaw in degrees (default: 0)
- `-r, --relative`: Move relative to current position

### Go To Examples

```text
# Go to position (1, 0, 0.5) over 2 seconds
cfcli hlc goto 1.0,0.0,0.5

# Go to position (1, 2, 1) with 90 degree yaw over 5 seconds
cfcli hlc goto 1.0,2.0,1.0 --yaw 90 -d 5.0

# Move 0.5 meters forward relative to current position
cfcli hlc goto 0.5,0,0 -r

# Negative coordinates are supported
cfcli hlc goto -1.0,-0.5,0.5

# Rotate to 180 degrees yaw while moving
cfcli hlc goto 0.0,0.0,0.5 --yaw 180
```

## Stop

Immediately stop all high-level commander operations and disable the motors.

```text
cfcli hlc stop
```

## Trajectory Operations

Trajectories allow complex pre-defined flight paths to be executed. Trajectories are defined
as polynomial segments in a YAML file, uploaded to the Crazyflie's memory, and then executed.

```text
Usage: cfcli hlc trajectory <COMMAND>

Commands:
  upload   Upload a trajectory from a YAML file
  run      Run a previously uploaded trajectory
  display  Display trajectory information (memory info or file contents)
```

### Trajectory File Format

Trajectory files are YAML files containing a list of polynomial segments. Each segment defines
a 7th-degree polynomial for x, y, z, and yaw over a specified duration. This format is compatible
with the output from the [uav_trajectories](https://github.com/whoenig/uav_trajectories) tool.

```yaml
segments:
  - duration: 1.5
    x: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    y: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    z: [0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    yaw: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
  - duration: 2.0
    x: [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    y: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    z: [0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    yaw: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
```

Each segment contains:
- `duration`: Time in seconds for this segment
- `x`, `y`, `z`: 8 polynomial coefficients (constant through 7th degree) for position in meters
- `yaw`: 8 polynomial coefficients for yaw angle in radians

The polynomial is evaluated as: `p(t) = c[0] + c[1]*t + c[2]*t^2 + ... + c[7]*t^7`

Each segment uses 132 bytes of memory on the Crazyflie.

### Upload Trajectory

Upload a trajectory from a YAML file to the Crazyflie's trajectory memory.

```text
cfcli hlc trajectory upload [ID] -i <FILE> [-s <OFFSET>]
```

Options:

- `ID` (positional): Trajectory ID to assign (default: 1)
- `-i, --input`: Path to the trajectory YAML file (required)
- `-s, --offset`: Memory offset in bytes (default: 0)

### Upload Examples

```text
# Upload trajectory with default ID (1)
cfcli hlc trajectory upload -i my_trajectory.yaml

# Upload trajectory with ID 2
cfcli hlc trajectory upload 2 -i figure8.yaml

# Upload multiple trajectories at different offsets
cfcli hlc trajectory upload 1 -i traj1.yaml -s 0
cfcli hlc trajectory upload 2 -i traj2.yaml -s 1000
```

### Run Trajectory

Execute a previously uploaded trajectory.

```text
cfcli hlc trajectory run <ID> [-f <time scale>] [-r] [-y] [--reversed]
```

Options:

- `-f, --time-scale`: Time scale factor (1.0 = normal, >1.0 = slower, <1.0 = faster)
- `-r, --relative-position`: Shift trajectory to start at current position
- `-y, --relative-yaw`: Align trajectory yaw to current heading
- `--reversed`: Run the trajectory backwards

### Run Examples

```text
# Run trajectory ID 1 at normal speed
cfcli hlc trajectory run 1

# Run trajectory at half speed
cfcli hlc trajectory run 1 -f 2.0

# Run trajectory at double speed
cfcli hlc trajectory run 1 -f 0.5

# Run trajectory relative to current position and yaw
cfcli hlc trajectory run 1 -r -y

# Run trajectory in reverse
cfcli hlc trajectory run 1 --reversed
```

### Display Trajectory Info

Display information about a trajectory file or the Crazyflie's trajectory memory.

```text
# Display trajectory file contents
cfcli hlc trajectory display my_trajectory.yaml

# Display trajectory memory info from Crazyflie
cfcli hlc trajectory display
```

## Complete Flight Example

Here's a complete example of a simple flight sequence:

```bash
# Select your Crazyflie
cfcli select

# Arm the motors
cfcli hlc arm

# Take off to 0.5 meters (default)
cfcli hlc takeoff

# Wait a moment (the command returns immediately)
sleep 3

# Move to a position
cfcli hlc goto 1.0,0.0,0.5 -d 2.0
sleep 3

# Move to another position
cfcli hlc goto 0.0,1.0,0.5 -d 2.0
sleep 3

# Return to origin
cfcli hlc goto 0.0,0.0,0.5 -d 2.0
sleep 3

# Land
cfcli hlc land
sleep 3

# Disarm (optional, landing auto-disarms after a delay)
cfcli hlc disarm
```

## Trajectory Flight Example

Example of uploading and running a trajectory:

```bash
# Select your Crazyflie
cfcli select

# Check trajectory file contents
cfcli hlc trajectory display figure8.yaml

# Arm and take off
cfcli hlc arm
cfcli hlc takeoff
sleep 3

# Upload the trajectory
cfcli hlc trajectory upload 1 -i figure8.yaml

# Run the trajectory (relative to current position)
cfcli hlc trajectory run 1 -r

# Wait for trajectory to complete, then land
sleep 10
cfcli hlc land
```
