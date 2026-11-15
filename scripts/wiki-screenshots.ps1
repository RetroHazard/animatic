<#
.SYNOPSIS
Regenerates the user-guide screenshots in wiki/images from a synthetic sample library.

.DESCRIPTION
Windows only (PowerShell 5.1 or 7). Builds the release executable and the
`docs-sample-library` example, writes the invented sample library to a temporary folder that is
mapped to a spare drive letter with `subst` (so every path in the pictures reads like
`S:\Anime\...`), then starts Animatic on it and drives the window through UI Automation (egui
exposes its widgets through AccessKit): it clicks pages, opens windows and menus and saves
each picture at the window's logical size (UI scale 1, so the result does not depend on the
display's scaling).

The sample settings work offline and never check for updates, so nothing is sent to AniDB or
GitHub. The app's PATH is reduced to the Windows folders so that no ffprobe on this machine
appears in the pictures. The script takes over the mouse and keyboard for about two minutes;
do not touch them while it runs. Afterwards the drive mapping, the temporary folder and the
processes are removed.

Every picture is written to -Out (default wiki/images) under the name the wiki pages use.
Review the changed pictures before committing them (git diff --stat wiki/images).

.PARAMETER Out
Folder the PNG files are written to.

.PARAMETER Only
Take only the pictures whose name matches one of these wildcards (for example 'options-*').

.PARAMETER SkipBuild
Use the existing release build.
#>
[CmdletBinding()]
param(
  [string]$Out = '',
  [string[]]$Only = @('*'),
  [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 3
if ($env:OS -ne 'Windows_NT') { throw 'wiki-screenshots.ps1 runs on Windows only.' }
$Repo = (Resolve-Path (Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..')).Path
if (-not $Out) { $Out = Join-Path $Repo 'wiki\images' }
$Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
New-Item -ItemType Directory -Force $Out | Out-Null

Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms, System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class AnimaticShots {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, IntPtr e);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint f, IntPtr e);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
}
'@
# Per-monitor DPI awareness: UI Automation, the cursor and the screen copy all use physical pixels.
[void][AnimaticShots]::SetProcessDpiAwarenessContext([IntPtr]::new(-4))
$UIA = [System.Windows.Automation.AutomationElement]
$Scope = [System.Windows.Automation.TreeScope]
$script:win = $null
$script:taken = New-Object System.Collections.Generic.List[string]

# ------------------------------------------------------------------------------- UI Automation

function All-Elements($root = $script:win) {
  $root.FindAll($Scope::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
}

function Test-Shown($el) {
  $r = $el.Current.BoundingRectangle
  -not $r.IsEmpty -and $r.Width -gt 0 -and -not [double]::IsInfinity($r.X)
}

# The script stays ASCII (Windows PowerShell reads a BOM-less file as ANSI): '...' in a name
# stands for the ellipsis character egui labels use.
function Find([string]$Name, [string]$Type = '', $Root = $script:win, [switch]$Like, [switch]$All, [switch]$Optional) {
  $Name = $Name.Replace('...', [string][char]0x2026)
  $found = @()
  for ($i = 0; $i -lt 30; $i++) {
    $found = @(All-Elements $Root | Where-Object {
        $n = $_.Current.Name
        ($(if ($Like) { $n -like $Name } else { $n -eq $Name })) -and
        (-not $Type -or $_.Current.ControlType.ProgrammaticName -eq "ControlType.$Type") -and (Test-Shown $_)
      })
    if ($found.Count -gt 0) { break }
    Start-Sleep -Milliseconds 200
  }
  if ($found.Count -eq 0) {
    if ($Optional) { return $null }
    throw "UI element not found: '$Name' ($Type)"
  }
  if ($All) { return $found }
  $found[0]
}

# The visible contents of the egui window titled $Title, or of the modal whose heading is $Title
# (modals expose no bounds of their own, so their contents stand in for them).
function Window([string]$Title, [int]$Tries = 50, [switch]$Optional) {
  $Title = $Title.Replace('...', [string][char]0x2026)
  $cond = New-Object System.Windows.Automation.PropertyCondition($UIA::ControlTypeProperty, [System.Windows.Automation.ControlType]::Window)
  for ($i = 0; $i -lt $Tries; $i++) {
    foreach ($w in $script:win.FindAll($Scope::Descendants, $cond)) {
      if ($w.Current.Name -like $Title -and (Test-Shown $w)) { return , @($w) }
      $kids = @(All-Elements $w | Where-Object { Test-Shown $_ })
      if ($w.Current.Name -eq '' -and ($kids | Where-Object { $_.Current.Name -like $Title })) { return , $kids }
    }
    Start-Sleep -Milliseconds 200
  }
  if ($Optional) { return $null }
  throw "window not found: '$Title'"
}

function Hwnd { [IntPtr]$script:win.Current.NativeWindowHandle }
function Scale { [AnimaticShots]::GetDpiForWindow((Hwnd)) / 96.0 }

function Client-Rect {
  $r = New-Object AnimaticShots+RECT; [void][AnimaticShots]::GetClientRect((Hwnd), [ref]$r)
  $p = New-Object AnimaticShots+POINT; [void][AnimaticShots]::ClientToScreen((Hwnd), [ref]$p)
  New-Object System.Drawing.Rectangle $p.X, $p.Y, $r.R, $r.B
}

function Move-To($el, [double]$dx = 0) {
  $r = $el.Current.BoundingRectangle
  [void][AnimaticShots]::SetCursorPos([int]($r.X + $r.Width / 2 + $dx), [int]($r.Y + $r.Height / 2))
}

function Click($el, [switch]$Right, [switch]$Ctrl, [double]$OffsetX = 0) {
  [void][AnimaticShots]::SetForegroundWindow((Hwnd))
  Move-To $el $OffsetX
  Start-Sleep -Milliseconds 150
  # egui reads the modifier from its own key events: give it a frame before and after the click.
  if ($Ctrl) { [AnimaticShots]::keybd_event(0x11, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 150 }
  $down, $up = if ($Right) { 0x8, 0x10 } else { 0x2, 0x4 }
  [AnimaticShots]::mouse_event($down, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 60
  [AnimaticShots]::mouse_event($up, 0, 0, 0, [IntPtr]::Zero)
  if ($Ctrl) { Start-Sleep -Milliseconds 150; [AnimaticShots]::keybd_event(0x11, 0, 2, [IntPtr]::Zero) }
  Start-Sleep -Milliseconds 500
}

function Hover($el) { Move-To $el; Start-Sleep -Milliseconds 700 }

function Drag($el, [int]$dx, [int]$dy) {
  Move-To $el; Start-Sleep -Milliseconds 150
  [AnimaticShots]::mouse_event(0x2, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 100
  $r = $el.Current.BoundingRectangle; $x = [int]($r.X + $r.Width / 2); $y = [int]($r.Y + $r.Height / 2)
  for ($i = 1; $i -le 10; $i++) {
    [void][AnimaticShots]::SetCursorPos($x + [int]($dx * $i / 10), $y + [int]($dy * $i / 10)); Start-Sleep -Milliseconds 30
  }
  Start-Sleep -Milliseconds 100; [AnimaticShots]::mouse_event(0x4, 0, 0, 0, [IntPtr]::Zero); Start-Sleep -Milliseconds 400
}

function Keys([string]$Keys) {
  [void][AnimaticShots]::SetForegroundWindow((Hwnd)); Start-Sleep -Milliseconds 150
  [System.Windows.Forms.SendKeys]::SendWait($Keys); Start-Sleep -Milliseconds 600
}

function Page([string]$Label) { Click (Find $Label 'Button' -Like); Start-Sleep -Milliseconds 400 }

# Choose $Menu > $Item and wait for the window $Title; a click that lands while the previous
# window is still closing is lost, so try once more.
function Menu([string]$Menu, [string]$Item, [string]$Title) {
  for ($attempt = 0; $attempt -lt 2; $attempt++) {
    Click (Find $Menu 'Button'); Click (Find $Item 'Button')
    if (Window $Title -Tries 25 -Optional) { return }
    Keys '{ESC}'
  }
  throw "window not found after $Menu > $($Item): '$Title'"
}

# Choose $Option in the combo box $Combo.
function Choose($Combo, [string]$Option) { Click $Combo; Click (Find $Option -Like) }

# The combo box on the same row as, and right of, the label $Label.
function Combo-After([string]$Label) {
  $l = (Find $Label 'Text').Current.BoundingRectangle
  Find '*' 'ComboBox' -Like -All | Where-Object {
    $r = $_.Current.BoundingRectangle; [Math]::Abs(($r.Y + $r.Height / 2) - ($l.Y + $l.Height / 2)) -lt $l.Height -and $r.X -gt $l.X
  } | Sort-Object { $_.Current.BoundingRectangle.X } | Select-Object -First 1
}

function Close-Windows { for ($i = 0; $i -lt 3; $i++) { Keys '{ESC}' } }

# ------------------------------------------------------------------------------------ pictures

function Wanted([string]$Name) { [bool]($Only | Where-Object { $Name -like $_ }) }

function Park-Mouse {
  # Over the title bar, where egui shows no hover text.
  $c = Client-Rect
  [void][AnimaticShots]::SetCursorPos($c.X + [int]($c.Width * 0.6), $c.Y - [int](12 * (Scale)))
  Start-Sleep -Milliseconds 600
}

# $Rect is in screen pixels. The window renders itself into the bitmap (PrintWindow with
# PW_RENDERFULLCONTENT), so notifications or other windows on top never end up in a picture.
function Save-Region([System.Drawing.Rectangle]$Rect, [string]$Name) {
  $wr = New-Object AnimaticShots+RECT; [void][AnimaticShots]::GetWindowRect((Hwnd), [ref]$wr)
  $full = New-Object System.Drawing.Bitmap ($wr.R - $wr.L), ($wr.B - $wr.T)
  $g = [System.Drawing.Graphics]::FromImage($full)
  $hdc = $g.GetHdc()
  $ok = [AnimaticShots]::PrintWindow((Hwnd), $hdc, 2)
  $g.ReleaseHdc($hdc); $g.Dispose()
  if (-not $ok) { throw 'PrintWindow failed' }
  $bmp = $full.Clone((New-Object System.Drawing.Rectangle ($Rect.X - $wr.L), ($Rect.Y - $wr.T), $Rect.Width, $Rect.Height), $full.PixelFormat)
  $full.Dispose()
  $s = Scale
  $w = [int][Math]::Round($Rect.Width / $s); $h = [int][Math]::Round($Rect.Height / $s)
  $outBmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($outBmp)
  $g.InterpolationMode = 'HighQualityBicubic'; $g.PixelOffsetMode = 'HighQuality'
  $g.DrawImage($bmp, 0, 0, $w, $h)
  $g.Dispose(); $bmp.Dispose()
  $outBmp.Save((Join-Path $Out "$Name.png"), [System.Drawing.Imaging.ImageFormat]::Png); $outBmp.Dispose()
  $script:taken.Add($Name)
  Write-Host "  $Name.png ($w x $h)"
}

# The whole window content (client area).
function Shot([string]$Name, [switch]$KeepMouse) {
  if (-not (Wanted $Name)) { return }
  if (-not $KeepMouse) { Park-Mouse }
  Save-Region (Client-Rect) $Name
}

# The region around $Elements, padded by $Pad points and clipped to the window content.
function Shot-Of($Elements, [string]$Name, [int]$Pad = 0, [switch]$KeepMouse) {
  if (-not (Wanted $Name)) { return }
  if (-not $KeepMouse) { Park-Mouse }
  $s = Scale; $c = Client-Rect
  $l = [double]::MaxValue; $t = [double]::MaxValue; $r = 0.0; $b = 0.0
  foreach ($e in @($Elements)) {
    if (-not (Test-Shown $e)) { continue }
    $x = $e.Current.BoundingRectangle
    $l = [Math]::Min($l, $x.Left); $t = [Math]::Min($t, $x.Top); $r = [Math]::Max($r, $x.Right); $b = [Math]::Max($b, $x.Bottom)
  }
  $p = $Pad * $s
  $l = [Math]::Max($c.Left, $l - $p); $t = [Math]::Max($c.Top, $t - $p)
  $r = [Math]::Min($c.Right, $r + $p); $b = [Math]::Min($c.Bottom, $b + $p)
  Save-Region (New-Object System.Drawing.Rectangle ([int]$l), ([int]$t), ([int]($r - $l)), ([int]($b - $t))) $Name
}

# Elements that appeared since $Before (a popup menu, a combo box list).
function New-Since($Before) {
  $known = @{}; foreach ($e in $Before) { $known[($e.GetRuntimeId() -join '.')] = 1 }
  @(All-Elements | Where-Object { (Test-Shown $_) -and -not $known.ContainsKey(($_.GetRuntimeId() -join '.')) })
}

# ------------------------------------------------------------------------- sample and process

function New-SampleCover([string]$Path, [string]$Title, [string]$Top, [string]$Bottom) {
  $w = 240; $h = 340
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'
  $rect = New-Object System.Drawing.Rectangle 0, 0, $w, $h
  $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, ([System.Drawing.ColorTranslator]::FromHtml($Top)), ([System.Drawing.ColorTranslator]::FromHtml($Bottom)), 90
  $g.FillRectangle($brush, $rect)
  $soft = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(60, 255, 255, 255))
  foreach ($c in @(@(170, 70, 120), @(30, 210, 70), @(150, 250, 46))) { $g.FillEllipse($soft, $c[0] - $c[2] / 2, $c[1] - $c[2] / 2, $c[2], $c[2]) }
  $font = New-Object System.Drawing.Font 'Segoe UI Semibold', 20
  $fmt = New-Object System.Drawing.StringFormat; $fmt.LineAlignment = 'Far'
  $g.DrawString($Title, $font, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF 16, 16, ($w - 32), ($h - 40)), $fmt)
  $g.DrawString('Animatic sample', (New-Object System.Drawing.Font 'Segoe UI', 9), $soft, 16, ($h - 26))
  $g.Dispose(); $bmp.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
}

# Invented covers, already "cached", so the detail window shows one without any download.
function New-SampleCovers([string]$Dir) {
  New-Item -ItemType Directory -Force $Dir | Out-Null
  $covers = @(
    @(90001, 'Sakura Signal', '#f4a6c0', '#7a2e55'), @(90002, 'Lanterns of Kiriyama', '#f6b75a', '#3b2140'),
    @(90003, 'Starfall Courier', '#5b7bd5', '#0f1633'), @(90004, 'Moss and Mortar', '#9cc58a', '#2f4a32'),
    @(90005, 'Ember Road', '#f08a4b', '#40171c'), @(90006, 'Paper Moon Radio', '#b9a4e0', '#2a2347'),
    @(90007, 'Clockwork Tanuki', '#d9b38c', '#4a3322'))
  foreach ($c in $covers) { New-SampleCover (Join-Path $Dir "sample-$($c[0]).png") $c[1] $c[2] $c[3] }
}

function New-Sample([string]$Root, [switch]$Fresh) {
  Get-ChildItem -Force $Root | Remove-Item -Recurse -Force
  $sampleArgs = @($Root); if ($Fresh) { $sampleArgs += '--fresh' }
  & $script:sampleExe @sampleArgs
  if ($LASTEXITCODE -ne 0) { throw 'docs-sample-library failed' }
  if (-not $Fresh) { New-SampleCovers (Join-Path $Root 'covers') }
}

function Start-Animatic([string]$Root) {
  $saved = @{ DB = $env:ANIMATIC_DB; SETTINGS = $env:ANIMATIC_SETTINGS; PATH = $env:PATH }
  $env:ANIMATIC_DB = Join-Path $Root 'animatic.sqlite3'
  $env:ANIMATIC_SETTINGS = Join-Path $Root 'settings.json'
  $env:PATH = "$env:SystemRoot\System32;$env:SystemRoot"
  try { $script:app = Start-Process -FilePath $script:appExe -PassThru }
  finally { $env:ANIMATIC_DB = $saved.DB; $env:ANIMATIC_SETTINGS = $saved.SETTINGS; $env:PATH = $saved.PATH }
  $cond = New-Object System.Windows.Automation.PropertyCondition($UIA::ProcessIdProperty, $script:app.Id)
  for ($i = 0; $i -lt 150 -and -not $script:win; $i++) {
    foreach ($w in $UIA::RootElement.FindAll($Scope::Children, $cond)) {
      if ($w.Current.Name -like 'Animatic*' -and $w.Current.BoundingRectangle.Width -gt 100) { $script:win = $w }
    }
    Start-Sleep -Milliseconds 200
  }
  if (-not $script:win) { throw 'the Animatic window did not appear' }
  [void][AnimaticShots]::SetForegroundWindow((Hwnd))
  [void](Find 'Overview' 'Button')
  Start-Sleep -Seconds 2  # the first library load
}

function Stop-Animatic {
  if ($script:app -and -not $script:app.HasExited) {
    Stop-Process -Id $script:app.Id -Force
    $script:app.WaitForExit()
  }
  $script:app = $null; $script:win = $null
}

# --------------------------------------------------------------------------------- the tour

function Tour-Fresh {
  Shot 'first-launch'
  Page 'Options'
  Shot-Of (Window 'Options') 'options-first-launch'
  Close-Windows
}

function Tour-Library {
  Keys '{F5}'
  Shot 'overview'
  Shot-Of ((Find 'Needs attention' 'Text'), (Find 'Hashing does not resume*' -Like), (Find 'Go online' 'Button')) 'overview-needs-attention' 12

  # Menus.
  foreach ($menu in @('File', 'Tools', 'Help')) {
    $before = All-Elements
    Click (Find $menu 'Button')
    Shot-Of (@(Find $menu 'Button') + (New-Since $before)) "menu-$($menu.ToLower())" 6 -KeepMouse
    Keys '{ESC}'
  }
  foreach ($sub in @(@('File', 'Import', 'menu-file-import'), @('File', 'Export', 'menu-file-export'),
      @('Tools', 'Edit AniDB data', 'menu-tools-edit-anidb'), @('Help', 'Keyboard shortcuts', 'menu-help-shortcuts'))) {
    $before = All-Elements
    Click (Find $sub[0] 'Button')
    Hover (Find $sub[1] 'Button'); Start-Sleep -Milliseconds 400
    Shot-Of (@(Find $sub[0] 'Button') + (New-Since $before)) $sub[2] 6 -KeepMouse
    Keys '{ESC}'; Keys '{ESC}'
  }

  # MyList: expand one anime, select an entry, open the editors.
  Page 'MyList'
  $title = Find 'Sakura Shingou' 'Hyperlink'
  Click $title -OffsetX (-($title.Current.BoundingRectangle.Width / 2) - 14 * (Scale))
  Click (Find '9' 'Button')
  Shot 'mylist'
  Click (Find 'Edit...' 'Button')
  Click (Find 'Change watched' 'CheckBox'); Click (Find 'Watched' 'CheckBox' -Root (Window 'Edit MyList entry')[0])
  Shot-Of (Window 'Edit MyList entry') 'mylist-edit-entry'
  Click (Find 'Cancel' 'Button')
  Click (Find 'Remove from MyList...' 'Button')
  Shot-Of (Window 'Remove from MyList') 'mylist-remove' 24
  Click (Find 'Cancel' 'Button')

  # Anime, the genre filter and the detail window.
  Page 'Anime'
  Shot 'anime'
  Choose (Combo-After 'Genres:') 'Fantasy (*'
  Shot 'anime-genre-filter'
  Click (Find 'Fantasy *' 'Button' -Like)
  Keys '^f'; Keys 'kiri'
  Shot 'anime-search'
  Keys '^f'; Keys '^a{BACKSPACE}'
  Click (Find 'Sakura Shingou' 'Button')
  $detail = Window 'Sakura Shingou'
  Start-Sleep -Seconds 1
  # The window opens short; make it tall enough for the cover and the open Episodes section.
  $edge = Find 'Resize window bottom edge' 'Separator' -Root $detail[0]
  Drag $edge 0 ((Client-Rect).Bottom - 40 * (Scale) - $edge.Current.BoundingRectangle.Y)
  Shot-Of (Window 'Sakura Shingou') 'anime-detail'
  foreach ($section in @('Episodes (*', 'Genres (*', 'Reviews (*')) { Click (Find $section -Like -Root $detail[0]) }
  Shot-Of (Window 'Sakura Shingou') 'anime-detail-sections'
  Close-Windows

  Page 'Advanced search'
  Click (Find 'In MyList' 'CheckBox')
  Shot 'advanced-search'
  Click (Find 'In MyList' 'CheckBox')

  # Hashed files and the actions on a selection.
  Page 'Hashed files'
  Shot 'hashed-files'
  # The list of queued files is open by default.
  Shot-Of ((Find 'Hash queue' 'Text'), (Find 'Hashing does not resume*' -Like), (Find '*damaged_download.mkv*' -Like -All), (Find '*Ember Road - 05.mkv*' -Like -All)) 'hash-queue' 8
  # Rows are virtualized: the search narrows the table to the files the next pictures use.
  Keys '^f'; Keys 'paper_moon'
  Click (Find 'hanabi_paper_moon_radio_01_720p.mkv' 'Button')
  Click (Find 'Rename...' 'Button')
  Shot-Of (Window 'Confirm rename') 'rename-confirm'
  Close-Windows
  foreach ($n in 2..6) { Click (Find "hanabi_paper_moon_radio_0$($n)_720p.mkv" 'Button') -Ctrl }
  Shot 'hashed-files-selection'
  Click (Find 'Rename * selected...' 'Button' -Like)
  Start-Sleep -Seconds 1
  Shot-Of (Window 'Batch rename') 'rename-batch'
  Close-Windows
  Click (Find 'Remove from list...' 'Button')
  Shot-Of (Window 'Remove from list') 'remove-from-list'
  Close-Windows
  $before = All-Elements
  $file = Find 'hanabi_paper_moon_radio_01_720p.mkv' 'Button'
  Click $file -Right
  Shot-Of (@($file) + (New-Since $before)) 'hashed-files-context-menu' 8 -KeepMouse
  Keys '{ESC}'
  $clear = Find 'Clear selection' 'Button' -Optional
  if ($clear) { Click $clear }
  Keys '^f'; Keys '^a'; Keys 'ember_road'
  Click (Find 'njf_lanterns_of_kiriyama_ember_road_01_1080p.mkv' 'Button')
  foreach ($n in 2..3) { Click (Find "njf_lanterns_of_kiriyama_ember_road_0$($n)_1080p.mkv" 'Button') -Ctrl }
  Click (Find 'Add to MyList...' 'Button')
  $add = Window 'Add to MyList'
  Click (Find 'Set watched' 'CheckBox' -Root $add[0])
  Shot-Of (Window 'Add to MyList') 'add-to-mylist'
  Close-Windows
  Keys '^f'; Keys '^a{BACKSPACE}'

  Page 'Groups'; Shot 'groups'
  Page 'Genres'; Shot 'genres'
  Page 'Queue*'; Shot 'queue'
  Click (Find 'Clear queue...' 'Button')
  Shot-Of (Window 'Clear the command queue?') 'queue-clear' 24
  Click (Find 'Cancel' 'Button')

  # Tools windows.
  Menu 'Tools' 'Request MyList export...' 'Request MyList export'
  Shot-Of (Window 'Request MyList export') 'mylist-export-request'
  Close-Windows
  Menu 'Tools' 'Full update...' 'Full update'
  Shot-Of (Window 'Full update') 'full-update' 24
  Click (Find 'Cancel' 'Button')
  Menu 'Tools' 'Compact library...' 'Compact library'
  Shot-Of (Window 'Compact library') 'compact-library' 24
  Click (Find 'Cancel' 'Button')
  Click (Find 'Tools' 'Button'); Click (Find 'Check library' 'Button')
  Start-Sleep -Seconds 2
  Page 'Log'; Shot 'log'

  # Options, page by page.
  Page 'Overview'
  Keys '^,'
  $pages = [ordered]@{ 'AniDB account' = 'options-account'; 'Appearance' = 'options-appearance'; 'Managed folders' = 'options-folders';
    'Hashing' = 'options-hashing'; 'Renaming' = 'options-renaming'; 'Players and tools' = 'options-players-tools';
    'Exports' = 'options-exports'; 'Updates' = 'options-updates' }
  foreach ($p in $pages.Keys) {
    Click (Find $p 'Button' -Root (Window 'Options')[0])
    Start-Sleep -Milliseconds 800  # ffprobe detection on Players and tools
    Shot-Of (Window 'Options') $pages[$p]
  }
  Click (Find 'Appearance' 'Button' -Root (Window 'Options')[0])
  Click (Find 'Customize status colors' -Like)
  Start-Sleep -Seconds 2  # the system tray check finishes in the background
  Shot-Of (Window 'Options') 'options-status-colors'
  Click (Find 'Customize status colors' -Like)

  Click (Find 'Managed folders' 'Button' -Root (Window 'Options')[0])
  Click (Find 'Downloads*' 'Button' -Root (Window 'Options')[0] -Like)
  Shot-Of (Window 'Options') 'options-folder-rule'
  Choose (Combo-After 'Auto rename') 'On'
  Start-Sleep -Seconds 2
  Shot-Of (Window 'Turn on automatic rename for ?Downloads??') 'auto-rename-confirm' 24
  Click (Find 'Cancel' 'Button')

  Click (Find 'Renaming' 'Button' -Root (Window 'Options')[0])
  $preset = Combo-After 'Naming convention'
  $before = All-Elements
  Click $preset
  Shot-Of (@($preset) + (New-Since $before)) 'options-renaming-conventions' 8 -KeepMouse
  Click (Find 'Custom pattern' -Like)
  Shot-Of (Window 'Options') 'options-renaming-custom'
  Click (Find 'Pattern reference' 'Button')
  Shot-Of (Window 'Renaming pattern reference') 'pattern-reference'
  Keys '{ESC}'
  Choose (Combo-After 'Naming convention') '`[Group`] Title - episode `[CRC`]'

  Click (Find 'Players and tools' 'Button' -Root (Window 'Options')[0])
  $install = Find 'Install ffprobe...' 'Button' -Optional
  if ($install) {
    Click $install
    Shot-Of (Window 'Install ffprobe') 'install-ffprobe' 24
    Click (Find 'Cancel' 'Button')
  }

  # A light theme picture for the Appearance page.
  Click (Find 'Appearance' 'Button' -Root (Window 'Options')[0])
  Choose (Combo-After 'Theme') 'Light'
  Close-Windows
  Page 'MyList'
  Shot 'theme-light'
  Keys '^,'
  Click (Find 'Appearance' 'Button' -Root (Window 'Options')[0])
  Choose (Combo-After 'Theme') 'Dark'
  Close-Windows

  # Help windows.
  Menu 'Help' 'About Animatic' 'About Animatic'
  Shot-Of (Window 'About Animatic') 'about'
  Click (Find 'Third-party notices' 'Button')
  Shot-Of (Window 'Third-party notices') 'third-party-notices'
  Close-Windows
}

# ------------------------------------------------------------------------------------- main

if (-not $SkipBuild) {
  Push-Location $Repo
  try {
    & cargo build --release -p animatic
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    & cargo build --release -p animatic-core --example docs-sample-library
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
  }
  finally { Pop-Location }
}
$script:appExe = Join-Path $Repo 'target\release\animatic.exe'
$script:sampleExe = Join-Path $Repo 'target\release\examples\docs-sample-library.exe'

$used = @(Get-PSDrive -PSProvider FileSystem | ForEach-Object { $_.Name }) + @((subst) | ForEach-Object { "$_"[0] })
$letter = @('S', 'T', 'R', 'Q', 'P', 'W', 'X', 'Y') | Where-Object { $used -notcontains $_ } | Select-Object -First 1
if (-not $letter) { throw 'no free drive letter for the sample library' }
$work = Join-Path ([System.IO.Path]::GetTempPath()) "animatic-wiki-shots-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
subst "$($letter):" $work
if ($LASTEXITCODE -ne 0) { throw "subst $($letter): failed" }
$root = "$($letter):\"
$script:app = $null
try {
  Write-Host 'First launch (empty library):'
  New-Sample $root -Fresh
  Start-Animatic $root; Tour-Fresh; Stop-Animatic
  Write-Host 'Sample library:'
  New-Sample $root
  Start-Animatic $root; Tour-Library; Stop-Animatic
}
finally {
  Stop-Animatic
  subst "$($letter):" /D | Out-Null
  Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
Write-Host "Saved $($script:taken.Count) picture(s) to $Out"
