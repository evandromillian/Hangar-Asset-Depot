<#
.SYNOPSIS
    Imports every subfolder of -Root as one asset (v1 = the folder's files) into a Hangar project.

.DESCRIPTION
    Each asset is cloned from -Template (an asset whose kind, description, tags and source are
    copied; its versions and files are not), or created from -Kind/-Tags/-Vendor/... when
    -Template is omitted. Assets go to -Project, to a new project (-NewProject, named
    -ProjectName), or by default to the template's project.

    Progress is kept in <Root>\.hangar-import.tsv (one asset id per folder), so re-running
    resumes without creating duplicates; folders already pushed print "nada mudou".

    -TextureSet links texture-set assets (ids, comma separated) to every version: models whose
    textures are not in their folder find them in the set by file name.

    -Files <wildcard> imports files instead of folders: each matching file directly in -Root
    becomes one asset (named after the file, without extension) whose v1 is that file alone.

.EXAMPLE
    .\tools\import-folders.ps1 -Root "C:\Dev\Assets\Weapons\Sci-Fi Swords" `
        -Template <asset-uuid> -AssetPrefix "Sci-Fi Sword "

.EXAMPLE
    .\tools\import-folders.ps1 -Root "C:\Dev\Assets\Spark City\Buildings" `
        -Template <asset-uuid> -AssetPrefix "Building " -TextureSet <texture-set-asset-uuid>

.EXAMPLE
    .\tools\import-folders.ps1 -Root "C:\Dev\Assets\Environment\Spark City\FBX" -Files "SparkCity_Sel_*.fbx" `
        -Template <asset-uuid> -TextureSet <texture-set-asset-uuid>
#>
param(
    [Parameter(Mandatory)] [string] $Root,
    [string] $Template,
    [string] $Project,
    [switch] $NewProject,
    [string] $ProjectName = (Split-Path $Root -Leaf),
    [string] $AssetPrefix = "",
    [string] $Kind = "model",
    [string[]] $Tags = @(),
    [string] $Vendor,
    [string] $Url,
    [string] $Author,
    [string] $License,
    [string] $Message = "Importação inicial",
    [string[]] $TextureSet = @(),
    [string] $Files,
    [switch] $Wait,
    [string] $Hangar = (Join-Path $PSScriptRoot "..\target\release\hangar.exe")
)

function Invoke-Hangar {
    $out = & $Hangar @args
    if ($LASTEXITCODE -ne 0) { throw "hangar $($args[0]) falhou (código $LASTEXITCODE)" }
    "$out".Trim()
}

if (-not (Test-Path $Root -PathType Container)) { throw "$Root não é uma pasta" }
if ($Project -and $NewProject) { throw "use -Project ou -NewProject, não ambos" }
$metaParams = "Kind", "Tags", "Vendor", "Url", "Author", "License" | Where-Object { $PSBoundParameters.ContainsKey($_) }
if ($Template -and $metaParams) { throw "-$($metaParams -join ', -') não se aplica com -Template (os dados vêm do template)" }

$stateFile = Join-Path $Root ".hangar-import.tsv"
$state = @{}
if (Test-Path $stateFile) {
    Get-Content $stateFile -Encoding UTF8 | Where-Object { $_ } | ForEach-Object {
        $k, $v = $_ -split "`t", 2
        $state[$k] = $v
    }
}
function Save-State($key, $id) {
    $state[$key] = $id
    Add-Content $stateFile "$key`t$id" -Encoding UTF8
}

# ---- project (none needed when cloning into the template's project)
$projectId = $state["@project"]
if ($Project -and $projectId -and $Project -ne $projectId) {
    throw "$stateFile já aponta para o projeto $projectId"
}
if (-not $projectId) {
    if ($Project) {
        $projectId = $Project
    } elseif ($NewProject -or -not $Template) {
        $projectId = Invoke-Hangar new-project --name $ProjectName
    }
    if ($projectId) { Save-State "@project" $projectId }
}
if ($projectId) { Write-Host "projeto $projectId" } else { Write-Host "projeto do template $Template" }

# ---- assets
if ($Template) {
    $create = @("clone-asset", $Template)
    if ($projectId) { $create += "--project", $projectId }
} else {
    $create = @("new-asset", $projectId, "--kind", $Kind)
    # `-File` invocations pass "a,b" as a single string.
    foreach ($t in $Tags -split ",") { if ($t.Trim()) { $create += "--tag", $t.Trim() } }
    if ($Vendor) { $create += "--vendor", $Vendor }
    if ($Url) { $create += "--url", $Url }
    if ($Author) { $create += "--author", $Author }
    if ($License) { $create += "--license", $License }
}
$pushArgs = @("-m", $Message, "--mirror")
if ($Wait) { $pushArgs += "--wait" }
foreach ($s in $TextureSet -split ",") { if ($s.Trim()) { $pushArgs += "--texture-set", $s.Trim() } }

# Items to import: (state key, asset name, folder pushed). With -Files each file is
# copied alone into a staging folder, so the version holds just that file.
if ($Files) {
    $staging = Join-Path ([System.IO.Path]::GetTempPath()) "hangar-import"
    $items = Get-ChildItem $Root -File -Filter $Files | Sort-Object Name | ForEach-Object {
        $dir = Join-Path $staging $_.BaseName
        Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
        New-Item -ItemType Directory $dir | Out-Null
        Copy-Item $_.FullName $dir
        [pscustomobject]@{ Key = $_.Name; Name = "$AssetPrefix$($_.BaseName)"; Dir = $dir }
    }
    $done = "arquivo(s) importado(s)"
} else {
    $items = Get-ChildItem $Root -Directory | Where-Object { -not $_.Name.StartsWith(".") } | Sort-Object Name |
        ForEach-Object { [pscustomobject]@{ Key = $_.Name; Name = "$AssetPrefix$($_.Name)"; Dir = $_.FullName } }
    $done = "pasta(s) importada(s)"
}
$items = @($items)
if (-not $items) { throw "nada para importar em $Root" }

$failed = @()
foreach ($item in $items) {
    $name = $item.Name
    try {
        $assetId = $state[$item.Key]
        if (-not $assetId) {
            $assetId = Invoke-Hangar @create --name $name
            Save-State $item.Key $assetId
        }
        Write-Host "`n== $name ($assetId)"
        & $Hangar push $assetId $item.Dir @pushArgs
        if ($LASTEXITCODE -ne 0) { throw "push falhou (código $LASTEXITCODE)" }
    } catch {
        Write-Warning "${name}: $_"
        $failed += $name
    }
}

Write-Host "`n$($items.Count - $failed.Count)/$($items.Count) $done"
if ($failed) {
    Write-Warning "falharam: $($failed -join ', ') — rode o script de novo para retomar"
    exit 1
}
if ($projectId) { Write-Host "acompanhe o processamento: & `"$Hangar`" assets $projectId" }
