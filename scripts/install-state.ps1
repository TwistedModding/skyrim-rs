# Copyright (c) 2026 TwistedModding. SPDX-License-Identifier: MPL-2.0-no-copyleft-exception
# This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of the MPL was not
# distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
# This Source Code Form is "Incompatible With Secondary Licenses", as defined by the Mozilla Public License, v. 2.0.
# One line per file in a game folder (path, size, time), to compare before and after a run. DevBench rewrites its own
# three files at every start.
param([Parameter(Mandatory)][string]$GameDir)
$start = $GameDir.Length + 1
Get-ChildItem $GameDir -Recurse -File -Force | ForEach-Object {
    $path = $_.FullName.Substring($start)
    if ($path -notmatch '^Data\\SKSE\\Plugins\\devbench\\(config|runtime|mcp-bridge)\.json$') { "$path|$($_.Length)|$($_.LastWriteTimeUtc.Ticks)" }
} | Sort-Object
