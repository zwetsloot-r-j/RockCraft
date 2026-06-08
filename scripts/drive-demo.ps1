<#
.SYNOPSIS
  Drive a running RockCraft editor over its WebSocket control channel.

.DESCRIPTION
  Connects to the loopback control server exposed by `rockcraft --control` and
  replays a paced sequence of edit actions so a human can watch the scenario on
  screen. Runs natively on Windows (uses .NET ClientWebSocket), so it can reach
  a Windows-hosted server on 127.0.0.1 — a WSL-side client cannot.

  Start the app in editor mode with the control server on a known port, e.g.:
      set ROCKCRAFT_CONTROL_ADDR=127.0.0.1:9123
      target-win\debug\rockcraft.exe --mock --control --edit
  then run this script.

.PARAMETER Port
  Control-server port (must match ROCKCRAFT_CONTROL_ADDR). Default 9123.

.PARAMETER PauseMs
  Delay between steps, in milliseconds, so the changes are watchable. Default 650.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\drive-demo.ps1 -Port 9123
#>
param(
    [int]$Port    = 9123,
    [int]$PauseMs = 650
)

$ErrorActionPreference = 'Stop'

$ws  = New-Object System.Net.WebSockets.ClientWebSocket
$uri = [System.Uri]::new("ws://127.0.0.1:$Port")
$ct  = [System.Threading.CancellationToken]::None
$ws.ConnectAsync($uri, $ct).Wait()
Write-Host "connected to $uri`n"

function Send-Frame([string]$payload) {
    $out = [System.ArraySegment[byte]]::new([System.Text.Encoding]::UTF8.GetBytes($payload))
    $ws.SendAsync($out, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, $ct).Wait()
    $buf = New-Object 'System.Byte[]' 65536
    $seg = [System.ArraySegment[byte]]::new($buf)
    $r   = $ws.ReceiveAsync($seg, $ct); $r.Wait()
    [System.Text.Encoding]::UTF8.GetString($buf, 0, $r.Result.Count)
}

function Act([int]$id, [string]$action, [hashtable]$params) {
    $payload = @{ type = 'run_action'; id = $id; action = $action; params = $params } | ConvertTo-Json -Compress
    $resp = Send-Frame $payload
    Write-Host ("{0,-22} -> {1}" -f $action, $resp)
    Start-Sleep -Milliseconds $PauseMs
}

function Query([int]$id, [string]$what) {
    # NOTE: `what` is PascalCase on the wire: State | Actions | Render.
    Send-Frame (@{ type = 'query'; id = $id; what = $what } | ConvertTo-Json -Compress)
}

# --- build a little melody ---
Act 1  'add_note'         @{}
Act 2  'cursor_right'     @{}
Act 3  'add_note'         @{}
Act 4  'cursor_right'     @{}
Act 5  'add_note'         @{}
Act 6  'cursor_up'        @{}
Act 7  'add_note'         @{}

# --- shape the last note: longer + louder ---
Act 8  'resize_note'      @{ delta_steps = 3 }
Act 9  'adjust_velocity'  @{ delta = 20 }

# --- grab it and drag it ---
Act 10 'toggle_grab'      @{}
Act 11 'cursor_right'     @{}
Act 12 'cursor_up'        @{}
Act 13 'toggle_grab'      @{}

# --- a chord ---
Act 14 'cursor_right'     @{}
Act 15 'enter_chord_mode' @{}
Act 16 'set_chord_degree' @{ degree = 5 }
Act 17 'commit_chord'     @{}

# --- selection + clipboard ---
Act 18 'cursor_to_start'  @{}
Act 19 'start_selection'  @{}
Act 20 'cursor_right'     @{}
Act 21 'cursor_right'     @{}
Act 22 'yank_selection'   @{}
Act 23 'cursor_to_end'    @{}
Act 24 'paste_clipboard'  @{}

# --- undo / redo ---
Act 25 'undo'             @{}
Act 26 'undo'             @{}
Act 27 'redo'             @{}

# --- playback toggles (audible if a SoundFont is configured) ---
Act 28 'toggle_metronome' @{}
Act 29 'play_from_start'  @{}
Start-Sleep -Milliseconds 1500
Act 30 'stop'             @{}

Write-Host "`n--- query: state snapshot ---"
Query 90 'State'
Write-Host "`n--- query: available actions ---"
Query 91 'Actions'

try { $ws.CloseAsync([System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure, 'done', $ct).Wait() } catch {}
Write-Host "`ndone."
