# Draws this app's icon and writes, next to this script:
#   app.ico            the app icon: rounded tile, multi-size PNG-in-ICO
#   store-logo.png     the Store's 1:1 logo, 1080 px, full-bleed square
#   icon-preview.png   the tile at 256 px, to look at
#   icon-preview-32.png  the tile at 32 px, to check it still reads when small
#   icon-64.rgba       the window icon, read by src/app.rs at compile time
#
#   pwsh -NoProfile -File assets/make-icon.ps1
#
# The build stage owns the two colours and Draw-Mark below. Everything under
# "Factory plumbing" stays as it is.

Add-Type -AssemblyName System.Drawing

# The tile's gradient, top-left to bottom-right: the app's accent, lighter to
# deeper. Keep them close in hue; the mark on top is white.
$TileTop    = [System.Drawing.Color]::FromArgb(255, 99, 102, 241)
$TileBottom = [System.Drawing.Color]::FromArgb(255, 55, 48, 163)

function New-RoundRect([single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
    $p = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $d = [math]::Min($r * 2, [math]::Min($w, $h))
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    $p
}

# Draws the mark: one simple white shape that says what the app does, made of
# a few thick filled forms so it survives at 16 px. No letters, no initials,
# no thin lines, no more than three elements.
#   $g      the Graphics to draw on (anti-aliased)
#   $left, $top, $size   the square the mark must stay inside
# Startup Monitor's mark: a speedometer gauge, the boot being timed.
# Three shapes: a thick open dial with round ends, a needle and its hub.
function Draw-Mark($g, [single]$left, [single]$top, [single]$size) {
    $white = [System.Drawing.Color]::White
    $w = [single]($size * 0.14)
    $pen = [System.Drawing.Pen]::new($white, $w)
    $pen.StartCap = 'Round'
    $pen.EndCap = 'Round'
    $inset = $size * 0.08
    $d = $size - 2 * $inset
    # The dial: 240 degrees, open at the bottom, centred a little low.
    $dialTop = $top + $inset + $size * 0.06
    $g.DrawArc($pen, $left + $inset, $dialTop, $d, $d, 150, 240)
    $cx = $left + $size / 2
    $cy = $dialTop + $d / 2
    # The needle, two thirds of the way round, pointing up and right.
    $a = -40 * [math]::PI / 180
    $len = $d * 0.36
    $needle = [System.Drawing.Pen]::new($white, [single]($size * 0.11))
    $needle.StartCap = 'Round'
    $needle.EndCap = 'Round'
    $g.DrawLine($needle, $cx, $cy, [single]($cx + [math]::Cos($a) * $len), [single]($cy + [math]::Sin($a) * $len))
    $hub = $size * 0.19
    $g.FillEllipse([System.Drawing.SolidBrush]::new($white), [single]($cx - $hub / 2), [single]($cy - $hub / 2), [single]$hub, [single]$hub)
    $pen.Dispose(); $needle.Dispose()
}

# ------------------------------------------------------- Factory plumbing

function New-IconBitmap([int]$size, [bool]$rounded) {
    $bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    $s = [single]$size
    $brush = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
        [System.Drawing.PointF]::new(0, 0), [System.Drawing.PointF]::new($s, $s), $TileTop, $TileBottom)
    if ($rounded) { $g.FillPath($brush, (New-RoundRect 0 0 $s $s ($s * 0.22))) }
    else { $g.FillRectangle($brush, 0, 0, $s, $s) }
    # The mark sits in the middle 60% of the tile, or 50% of the full-bleed
    # logo, which the Store crops and overlays more tightly.
    $span = if ($rounded) { 0.60 } else { 0.50 }
    $markSize = $s * $span
    $origin = ($s - $markSize) / 2
    Draw-Mark $g ([single]$origin) ([single]$origin) ([single]$markSize)
    $g.Dispose()
    $bmp
}

function Get-Scaled([int]$size) {
    # Draw large and scale down so small sizes stay smooth.
    $big = New-IconBitmap ([math]::Max($size * 4, 256)) $true
    $small = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($small)
    $g.InterpolationMode = 'HighQualityBicubic'
    $g.PixelOffsetMode = 'HighQuality'
    $g.DrawImage($big, 0, 0, $size, $size)
    $g.Dispose(); $big.Dispose()
    $small
}

function Get-PngBytes($bmp) {
    $ms = [IO.MemoryStream]::new()
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    , $ms.ToArray()
}

$out = $PSScriptRoot
$png = [System.Drawing.Imaging.ImageFormat]::Png

$logo = New-IconBitmap 1080 $false
$logo.Save((Join-Path $out 'store-logo.png'), $png); $logo.Dispose()
foreach ($preview in @(@(256, 'icon-preview.png'), @(32, 'icon-preview-32.png'))) {
    $bmp = Get-Scaled $preview[0]
    $bmp.Save((Join-Path $out $preview[1]), $png); $bmp.Dispose()
}

# The window's own icon (title bar and taskbar), compiled into the exe as raw
# RGBA: 64 x 64, four bytes a pixel.
$win = Get-Scaled 64
$rgba = [byte[]]::new(64 * 64 * 4)
for ($y = 0; $y -lt 64; $y++) {
    for ($x = 0; $x -lt 64; $x++) {
        $c = $win.GetPixel($x, $y); $i = ($y * 64 + $x) * 4
        $rgba[$i] = $c.R; $rgba[$i + 1] = $c.G; $rgba[$i + 2] = $c.B; $rgba[$i + 3] = $c.A
    }
}
$win.Dispose()
[IO.File]::WriteAllBytes((Join-Path $out 'icon-64.rgba'), $rgba)

$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
$images = foreach ($size in $sizes) {
    $bmp = Get-Scaled $size
    $bytes = Get-PngBytes $bmp
    $bmp.Dispose()
    , $bytes
}
$stream = [IO.MemoryStream]::new()
$w = [IO.BinaryWriter]::new($stream)
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$images[$i].Length); $w.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($bytes in $images) { $w.Write($bytes) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $out 'app.ico'), $stream.ToArray())
Write-Host "Wrote app.ico, store-logo.png, icon-preview.png and icon-preview-32.png to $out"
