# Regenerates assets/app.ico and assets/paused.ico (a screen with a cursor on it).
# Run from the repo root: powershell -ExecutionPolicy Bypass -File scripts/make-icons.ps1
Add-Type -AssemblyName System.Drawing

function New-IconImage([int]$size, [System.Drawing.Color]$screen) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.Clear([System.Drawing.Color]::Transparent)
    $s = $size / 32.0

    # Screen body and stand.
    $body = New-Object System.Drawing.Drawing2D.GraphicsPath
    $r = 4 * $s; $x = 1 * $s; $y = 3 * $s; $w = 30 * $s; $h = 20 * $s
    $body.AddArc($x, $y, $r, $r, 180, 90); $body.AddArc($x + $w - $r, $y, $r, $r, 270, 90)
    $body.AddArc($x + $w - $r, $y + $h - $r, $r, $r, 0, 90); $body.AddArc($x, $y + $h - $r, $r, $r, 90, 90)
    $body.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush $screen), $body)
    $standBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 90, 96, 110))
    $g.FillRectangle($standBrush, 13 * $s, 23 * $s, 6 * $s, 4 * $s)
    $g.FillRectangle($standBrush, 9 * $s, 27 * $s, 14 * $s, 2.5 * $s)

    # Cursor arrow.
    $pts = @(
        (New-Object System.Drawing.PointF (12 * $s), (6 * $s)),
        (New-Object System.Drawing.PointF (12 * $s), (20 * $s)),
        (New-Object System.Drawing.PointF (15.5 * $s), (16.8 * $s)),
        (New-Object System.Drawing.PointF (18 * $s), (22 * $s)),
        (New-Object System.Drawing.PointF (20.2 * $s), (21 * $s)),
        (New-Object System.Drawing.PointF (17.8 * $s), (15.8 * $s)),
        (New-Object System.Drawing.PointF (22.5 * $s), (15.5 * $s))
    )
    $g.FillPolygon([System.Drawing.Brushes]::White, $pts)
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 20, 22, 28)), ([Math]::Max(1.0, 1.2 * $s))
    $pen.LineJoin = 'Round'
    $g.DrawPolygon($pen, $pts)
    $g.Dispose()
    return $bmp
}

function Write-Ico([string]$path, [System.Drawing.Color]$screen) {
    $sizes = 16, 20, 24, 32, 40, 48, 64, 256
    # 256px is stored as PNG; smaller frames as 32-bit DIBs, which every icon loader reads.
    $pngs = foreach ($sz in $sizes) {
        $bmp = New-IconImage $sz $screen
        $ms = New-Object System.IO.MemoryStream
        if ($sz -ge 256) {
            $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
        } else {
            $w = New-Object System.IO.BinaryWriter $ms
            $w.Write([UInt32]40); $w.Write([Int32]$sz); $w.Write([Int32]($sz * 2))
            $w.Write([UInt16]1); $w.Write([UInt16]32); $w.Write([UInt32]0)
            $maskStride = [int][Math]::Ceiling($sz / 32.0) * 4
            $w.Write([UInt32]($sz * $sz * 4 + $maskStride * $sz))
            $w.Write([Int32]0); $w.Write([Int32]0); $w.Write([UInt32]0); $w.Write([UInt32]0)
            for ($y = $sz - 1; $y -ge 0; $y--) {
                for ($x = 0; $x -lt $sz; $x++) {
                    $c = $bmp.GetPixel($x, $y)
                    $w.Write([byte]$c.B); $w.Write([byte]$c.G); $w.Write([byte]$c.R); $w.Write([byte]$c.A)
                }
            }
            $w.Write((New-Object byte[] ($maskStride * $sz)))
            $w.Flush()
        }
        , $ms.ToArray()
    }
    $out = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter $out
    $bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $dim = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
        $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
        $bw.Write([UInt16]1); $bw.Write([UInt16]32)
        $bw.Write([UInt32]$pngs[$i].Length); $bw.Write([UInt32]$offset)
        $offset += $pngs[$i].Length
    }
    foreach ($p in $pngs) { $bw.Write($p) }
    $bw.Flush()
    [System.IO.File]::WriteAllBytes($path, $out.ToArray())
}

$root = Split-Path -Parent $PSScriptRoot
Write-Ico (Join-Path $root 'assets\app.ico') ([System.Drawing.Color]::FromArgb(255, 88, 101, 242))
Write-Ico (Join-Path $root 'assets\paused.ico') ([System.Drawing.Color]::FromArgb(255, 120, 124, 135))
Write-Host 'Wrote assets\app.ico and assets\paused.ico'
