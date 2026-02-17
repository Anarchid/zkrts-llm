-- Agent Combat Widget
-- Accumulates damage/kill events Lua-side with spatial clustering.
-- Provides a drain-on-read HUD (clears on read) and a persistent details query.

function widget:GetInfo()
    return {
        name    = "Agent Combat",
        desc    = "Combat log with damage accumulation and spatial clustering for agent awareness",
        author  = "afcomech",
        version = "0.1",
        date    = "2026",
        license = "MIT",
        layer   = 1,
        enabled = true,
    }
end

--------------------------------------------------------------------------------
-- Configuration
--------------------------------------------------------------------------------

local CLUSTER_RADIUS = 600
local PRUNE_INTERVAL = 150   -- frames between stale cluster pruning (~5s)
local STALE_THRESHOLD = 900  -- frames of inactivity before pruning (~30s)

--------------------------------------------------------------------------------
-- State
--------------------------------------------------------------------------------

local myTeamID = nil
local myAllyTeamID = nil

-- Accumulator (drained on HUD read)
local combatLog = {
    ourParticipants = {},    -- defName -> count of unique unitIDs
    theirParticipants = {},  -- defName -> count of unique unitIDs
    ourCasualties = {},      -- defName -> count killed
    theirCasualties = {},    -- defName -> count killed
    damageIn = 0,            -- total damage received
    damageOut = 0,           -- total damage dealt
    startFrame = 0,
}

local ourSeenIDs = {}        -- unitID -> defName (dedup participants)
local theirSeenIDs = {}      -- unitID -> defName

-- Spatial clusters (persistent, pruned on staleness)
local clusters = {}  -- { cx, cz, ourUnits={defName->count}, theirUnits={defName->count}, ourLosses={defName->count}, theirLosses={defName->count}, damageIn, damageOut, lastFrame }

--------------------------------------------------------------------------------
-- Helpers
--------------------------------------------------------------------------------

local function getDefName(unitDefID)
    if unitDefID and UnitDefs[unitDefID] then
        return UnitDefs[unitDefID].name
    end
    return "unknown"
end

local function findNearestCluster(x, z)
    local bestDist = CLUSTER_RADIUS * CLUSTER_RADIUS
    local bestIdx = nil
    for i, c in ipairs(clusters) do
        local dx = c.cx - x
        local dz = c.cz - z
        local dist2 = dx * dx + dz * dz
        if dist2 < bestDist then
            bestDist = dist2
            bestIdx = i
        end
    end
    return bestIdx
end

local function getOrCreateCluster(x, z, frame)
    local idx = findNearestCluster(x, z)
    if idx then
        local c = clusters[idx]
        -- Update running average centroid
        c.cx = (c.cx + x) * 0.5
        c.cz = (c.cz + z) * 0.5
        c.lastFrame = frame
        return c
    end
    -- Create new cluster
    local c = {
        cx = x, cz = z,
        ourUnits = {}, theirUnits = {},
        ourLosses = {}, theirLosses = {},
        damageIn = 0, damageOut = 0,
        lastFrame = frame,
    }
    clusters[#clusters + 1] = c
    return c
end

local function incMap(tbl, key, amount)
    tbl[key] = (tbl[key] or 0) + (amount or 1)
end

local function formatDefCounts(tbl)
    local parts = {}
    for defName, count in pairs(tbl) do
        parts[#parts + 1] = count .. "x " .. defName
    end
    return table.concat(parts, ", ")
end

local function isTableEmpty(tbl)
    for _ in pairs(tbl) do return false end
    return true
end

local function hasActivity()
    return combatLog.damageIn > 0 or combatLog.damageOut > 0
        or not isTableEmpty(combatLog.ourCasualties)
        or not isTableEmpty(combatLog.theirCasualties)
end

local function clearAccumulator()
    combatLog.ourParticipants = {}
    combatLog.theirParticipants = {}
    combatLog.ourCasualties = {}
    combatLog.theirCasualties = {}
    combatLog.damageIn = 0
    combatLog.damageOut = 0
    combatLog.startFrame = Spring.GetGameFrame()
    ourSeenIDs = {}
    theirSeenIDs = {}
end

--------------------------------------------------------------------------------
-- Tool handlers
--------------------------------------------------------------------------------

local function handleToolCall(toolName, args)
    if toolName == "combat:hud" then
        -- Drain-on-read: return summary then clear
        if not hasActivity() then
            return { type = "text", text = "quiet" }
        end

        local parts = {}

        -- Participants
        local ourParts = formatDefCounts(combatLog.ourParticipants)
        local theirParts = formatDefCounts(combatLog.theirParticipants)
        parts[#parts + 1] = "COMBAT!"
        if ourParts ~= "" then
            parts[#parts + 1] = "Ours [" .. ourParts .. "]"
        end
        if theirParts ~= "" then
            parts[#parts + 1] = "vs Theirs [" .. theirParts .. "]"
        end

        -- Casualties
        local ourLoss = formatDefCounts(combatLog.ourCasualties)
        local theirLoss = formatDefCounts(combatLog.theirCasualties)
        if ourLoss ~= "" then
            parts[#parts + 1] = "| LOST: " .. ourLoss
        end
        if theirLoss ~= "" then
            parts[#parts + 1] = "| KILLED: " .. theirLoss
        end

        -- Damage totals
        parts[#parts + 1] = string.format("| dmg in/out: %.0f/%.0f", combatLog.damageIn, combatLog.damageOut)

        local text = table.concat(parts, " ")

        -- Drain
        clearAccumulator()

        return { type = "text", text = text }

    elseif toolName == "combat:details" then
        -- Read-only (no drain). JSON array of spatial clusters.
        local results = {}
        for _, c in ipairs(clusters) do
            local ourParts = {}
            for defName, count in pairs(c.ourUnits) do
                ourParts[#ourParts + 1] = string.format('"%s":%d', defName, count)
            end
            local theirParts = {}
            for defName, count in pairs(c.theirUnits) do
                theirParts[#theirParts + 1] = string.format('"%s":%d', defName, count)
            end
            local ourLoss = {}
            for defName, count in pairs(c.ourLosses) do
                ourLoss[#ourLoss + 1] = string.format('"%s":%d', defName, count)
            end
            local theirLoss = {}
            for defName, count in pairs(c.theirLosses) do
                theirLoss[#theirLoss + 1] = string.format('"%s":%d', defName, count)
            end
            results[#results + 1] = string.format(
                '{"cx":%.0f,"cz":%.0f,"ourUnits":{%s},"theirUnits":{%s},"ourLosses":{%s},"theirLosses":{%s},"damageIn":%.0f,"damageOut":%.0f}',
                c.cx, c.cz,
                table.concat(ourParts, ","),
                table.concat(theirParts, ","),
                table.concat(ourLoss, ","),
                table.concat(theirLoss, ","),
                c.damageIn, c.damageOut
            )
        end
        return {
            type = "text",
            text = string.format('{"clusters":[%s]}', table.concat(results, ",")),
        }

    elseif toolName == "combat:desc" then
        return {
            type = "text",
            text = "Combat log tracks all damage and kills between your units and enemies. " ..
                   "The combat:hud is drain-on-read: each call returns everything since the last read, then clears. " ..
                   "Enable it as a HUD overlay for automatic battle awareness every inference cycle. " ..
                   "'quiet' means no combat occurred since the last read. " ..
                   "combat:details gives spatial clusters of fighting (read-only, no drain) showing where battles happened and per-location casualties. " ..
                   "Use it after a major engagement to understand what happened where.",
        }
    end

    return { type = "text", text = '{"error":"unknown tool"}' }
end

--------------------------------------------------------------------------------
-- Widget lifecycle
--------------------------------------------------------------------------------

function widget:Initialize()
    myTeamID = Spring.GetMyTeamID()
    myAllyTeamID = Spring.GetMyAllyTeamID()
    combatLog.startFrame = Spring.GetGameFrame()

    if WG.AgentTools and WG.AgentTools.Register then
        WG.AgentTools.Register("combat", {
            {
                name = "combat:hud",
                description = "Drain-on-read combat summary. Returns battle participants, casualties, and damage since last read, then clears. Returns 'quiet' if no combat occurred.",
                inputSchema = { type = "object" },
            },
            {
                name = "combat:details",
                description = "Read-only spatial clusters of combat activity showing per-location participants, casualties, and damage.",
                inputSchema = { type = "object" },
            },
            {
                name = "combat:desc",
                description = "Usage guide for combat tools.",
                inputSchema = { type = "object" },
            },
        }, handleToolCall)
    else
        Spring.Echo("[AgentCombat] WG.AgentTools not available -- tools not registered")
    end
end

--------------------------------------------------------------------------------
-- Engine callins
--------------------------------------------------------------------------------

function widget:UnitDamaged(unitID, unitDefID, unitTeam, damage, paralyzer, weaponDefID, projectileID, attackerID, attackerDefID, attackerTeam)
    if paralyzer then return end
    if not attackerID then return end
    if damage <= 0 then return end

    local frame = Spring.GetGameFrame()
    local defName = getDefName(unitDefID)
    local attackerDefName = getDefName(attackerDefID)

    -- Get position of the damaged unit for clustering
    local x, _, z = Spring.GetUnitPosition(unitID)
    if not x then return end

    local isOurUnit = (unitTeam == myTeamID)
    local isOurAttacker = (attackerTeam == myTeamID)

    -- We only care about combat involving us
    if not isOurUnit and not isOurAttacker then return end

    local cluster = getOrCreateCluster(x, z, frame)

    if isOurUnit then
        -- We took damage
        combatLog.damageIn = combatLog.damageIn + damage
        cluster.damageIn = cluster.damageIn + damage

        -- Track our unit as participant
        if not ourSeenIDs[unitID] then
            ourSeenIDs[unitID] = defName
            incMap(combatLog.ourParticipants, defName)
            incMap(cluster.ourUnits, defName)
        end
        -- Track attacker as enemy participant
        if attackerID and not theirSeenIDs[attackerID] then
            theirSeenIDs[attackerID] = attackerDefName
            incMap(combatLog.theirParticipants, attackerDefName)
            incMap(cluster.theirUnits, attackerDefName)
        end
    elseif isOurAttacker then
        -- We dealt damage
        combatLog.damageOut = combatLog.damageOut + damage
        cluster.damageOut = cluster.damageOut + damage

        -- Track our attacker as participant
        if not ourSeenIDs[attackerID] then
            ourSeenIDs[attackerID] = attackerDefName
            incMap(combatLog.ourParticipants, attackerDefName)
            incMap(cluster.ourUnits, attackerDefName)
        end
        -- Track enemy as participant
        if not theirSeenIDs[unitID] then
            theirSeenIDs[unitID] = defName
            incMap(combatLog.theirParticipants, defName)
            incMap(cluster.theirUnits, defName)
        end
    end
end

function widget:UnitDestroyed(unitID, unitDefID, unitTeam, attackerID, attackerDefID, attackerTeam)
    if not attackerID then return end

    local defName = getDefName(unitDefID)
    local frame = Spring.GetGameFrame()

    local isOurUnit = (unitTeam == myTeamID)
    local isOurAttacker = (attackerTeam == myTeamID)

    if not isOurUnit and not isOurAttacker then return end

    -- Get position for clustering (use last known or attacker position)
    local x, _, z = Spring.GetUnitPosition(unitID)
    if not x and attackerID then
        x, _, z = Spring.GetUnitPosition(attackerID)
    end

    if isOurUnit and attackerTeam and attackerTeam ~= myTeamID then
        -- Our unit killed in combat
        incMap(combatLog.ourCasualties, defName)
        if x then
            local cluster = getOrCreateCluster(x, z, frame)
            incMap(cluster.ourLosses, defName)
        end
    elseif isOurAttacker and unitTeam ~= myTeamID then
        -- Enemy killed by us
        incMap(combatLog.theirCasualties, defName)
        if x then
            local cluster = getOrCreateCluster(x, z, frame)
            incMap(cluster.theirLosses, defName)
        end
    end
end

function widget:GameFrame(frame)
    if frame % PRUNE_INTERVAL ~= 0 then return end

    -- Prune stale clusters
    local i = 1
    while i <= #clusters do
        if frame - clusters[i].lastFrame > STALE_THRESHOLD then
            table.remove(clusters, i)
        else
            i = i + 1
        end
    end
end
