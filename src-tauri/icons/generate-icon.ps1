Add-Type -AssemblyName System.Drawing

$size = 1024
$bmp = New-Object System.Drawing.Bitmap $size, $size
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$gfx.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$gfx.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$gfx.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
$gfx.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality

$gfx.Clear([System.Drawing.Color]::Transparent)

$radius = 200
$rect = New-Object System.Drawing.Rectangle 0, 0, ($size - 1), ($size - 1)
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$path.AddArc($rect.X, $rect.Y, ($radius * 2), ($radius * 2), 180, 90)
$path.AddArc($rect.Right - $radius * 2, $rect.Y, ($radius * 2), ($radius * 2), 270, 90)
$path.AddArc($rect.Right - $radius * 2, $rect.Bottom - $radius * 2, ($radius * 2), ($radius * 2), 0, 90)
$path.AddArc($rect.X, $rect.Bottom - $radius * 2, ($radius * 2), ($radius * 2), 90, 90)
$path.CloseFigure()

$c1 = [System.Drawing.Color]::FromArgb(255, 0xFF, 0x20, 0x50)
$c2 = [System.Drawing.Color]::FromArgb(255, 0xFF, 0x44, 0x77)
$c3 = [System.Drawing.Color]::FromArgb(255, 0xC8, 0x22, 0x8A)

$brushRect = New-Object System.Drawing.RectangleF 0, 0, $size, $size
$brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $brushRect, $c1, $c3, 135
$blend = New-Object System.Drawing.Drawing2D.ColorBlend 3
$blend.Colors = @($c1, $c2, $c3)
$blend.Positions = @(0.0, 0.5, 1.0)
$brush.InterpolationColors = $blend

$gfx.FillPath($brush, $path)

$innerRect = New-Object System.Drawing.RectangleF 0, 0, $size, ($size / 3)
$highlightColor1 = [System.Drawing.Color]::FromArgb(64, 255, 255, 255)
$highlightColor2 = [System.Drawing.Color]::FromArgb(0, 255, 255, 255)
$highlightBrush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $innerRect, $highlightColor1, $highlightColor2, 90
$gfx.SetClip($path)
$gfx.FillRectangle($highlightBrush, $innerRect)
$gfx.ResetClip()

$cx = $size / 2.0
$cy = $size / 2.0 + 30
$triW = $size * 0.5
$triH = $triW * 0.78
$pts = @(
    [System.Drawing.PointF]::new($cx - $triW / 2, $cy - $triH / 2),
    [System.Drawing.PointF]::new($cx + $triW / 2, $cy - $triH / 2),
    [System.Drawing.PointF]::new($cx, $cy + $triH / 2)
)
$triPath = New-Object System.Drawing.Drawing2D.GraphicsPath
$triPath.AddPolygon([System.Drawing.PointF[]]$pts)
$triPath.CloseFigure()

$shadowColor = [System.Drawing.Color]::FromArgb(80, 0, 0, 0)
$shadowBrush = New-Object System.Drawing.SolidBrush $shadowColor
$shadowMatrix = New-Object System.Drawing.Drawing2D.Matrix
$shadowMatrix.Translate(0, 8)
$shadowPath = $triPath.Clone()
$shadowPath.Transform($shadowMatrix)
$gfx.FillPath($shadowBrush, $shadowPath)

$triBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
$gfx.FillPath($triBrush, $triPath)

$outPath = "C:\Users\megakim\Documents\YouTubeDownloader-Basic\src-tauri\icons\source.png"
$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)

$gfx.Dispose()
$bmp.Dispose()

"saved: $outPath"
