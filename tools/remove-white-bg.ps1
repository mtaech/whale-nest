# Remove white background from a PNG + clean edge halos.
# Usage: powershell -File remove-white-bg.ps1 -InPath <png> -OutPath <png>
#   -Threshold : RGB components above this are candidates for white (default 235)
#   -Feather   : edge pixels are made partially transparent so no white fringe remains

param(
    [Parameter(Mandatory = $true)][string]$InPath,
    [Parameter(Mandatory = $true)][string]$OutPath,
    [int]$Threshold = 235,
    [switch]$Quiet
)

Add-Type -AssemblyName System.Drawing

function Log($msg) {
    if (-not $Quiet) { Write-Host $msg }
}

$src = [System.Drawing.Bitmap]::FromFile((Resolve-Path $InPath))
$w = $src.Width
$h = $src.Height
Log "source: $w x $h ($($src.PixelFormat))"

# Work in 32bpp ARGB for predictable byte layout
$bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.DrawImage($src, 0, 0, $w, $h)
$g.Dispose()
$src.Dispose()

$rect = New-Object System.Drawing.Rectangle 0, 0, $w, $h
$data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadWrite, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$stride = $data.Stride
$bytes = New-Object byte[] ($stride * $h)
[System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $bytes, 0, $bytes.Length)

# Pass 1: mark pure-white-ish pixels (fully opaque white) as transparent.
$isWhite = New-Object bool[] ($w * $h)
for ($y = 0; $y -lt $h; $y++) {
    $row = $y * $stride
    for ($x = 0; $x -lt $w; $x++) {
        $i = $row + $x * 4
        $b = $bytes[$i]; $gg = $bytes[$i + 1]; $r = $bytes[$i + 2]; $a = $bytes[$i + 3]
        if ($a -ge 200 -and $r -ge $Threshold -and $gg -ge $Threshold -and $b -ge $Threshold) {
            $isWhite[$y * $w + $x] = $true
            $bytes[$i + 3] = 0
        }
    }
}

# Pass 2: edge decontamination - semi-white pixels adjacent to the now-transparent
# region and to opaque content get alpha scaled down to kill the white fringe.
for ($y = 0; $y -lt $h; $y++) {
    $row = $y * $stride
    for ($x = 0; $x -lt $w; $x++) {
        $i = $row + $x * 4
        $b = $bytes[$i]; $gg = $bytes[$i + 1]; $r = $bytes[$i + 2]; $a = $bytes[$i + 3]
        if ($a -eq 0) { continue }
        # how "white" is this pixel?
        $minc = [math]::Min($r, [math]::Min($gg, $b))
        $maxc = [math]::Max($r, [math]::Max($gg, $b))
        $nearWhite = ($minc -ge ($Threshold - 30)) -and ($maxc -le 255) -and (($maxc - $minc) -le 25)
        if (-not $nearWhite) { continue }
        # does it border a fully transparent pixel (the cut background)?
        $touchesTransparent = $false
        if ($x -gt 0 -and $isWhite[$y * $w + ($x - 1)]) { $touchesTransparent = $true }
        if (-not $touchesTransparent -and $x -lt ($w - 1) -and $isWhite[$y * $w + ($x + 1)]) { $touchesTransparent = $true }
        if (-not $touchesTransparent -and $y -gt 0 -and $isWhite[($y - 1) * $w + $x]) { $touchesTransparent = $true }
        if (-not $touchesTransparent -and $y -lt ($h - 1) -and $isWhite[($y + 1) * $w + $x]) { $touchesTransparent = $true }
        if ($touchesTransparent) {
            # white fringe: reduce alpha roughly proportional to whiteness
            $whiteness = ($r + $gg + $b) / 3.0
            $newA = [int]($a * (1.0 - (($whiteness - ($Threshold - 40)) / 255.0)))
            $bytes[$i + 3] = [byte][math]::Max(0, [math]::Min(255, $newA))
        }
    }
}

[System.Runtime.InteropServices.Marshal]::Copy($bytes, 0, $data.Scan0, $bytes.Length)
$bmp.UnlockBits($data)

# Save as PNG with alpha
$outDir = Split-Path -Parent $OutPath
if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }
$bmp.Save($OutPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Log "wrote: $OutPath"
