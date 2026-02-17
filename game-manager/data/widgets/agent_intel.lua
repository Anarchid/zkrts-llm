-- Agent Intel Widget
-- Tracks enemy last-known positions via UnitEnteredLos/UnitLeftLos callins.
-- Maintains a scouting map with per-sector staleness.

function widget:GetInfo()
    return {
        name    = "Agent Intel",
        desc    = "Enemy tracking and scouting map for agent decision-making",
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

local SECTOR_SIZE = 512    -- scouting sector size in world units
local DECAY_RATE  = 0.001  -- confidence decay per frame

--------------------------------------------------------------------------------
-- State
--------------------------------------------------------------------------------

local knownEnemies = {}    -- unitID -> { defID, name, x, z, lastSeen, confidence, inLos }
local scoutMap = {}        -- [sz][sx] = { lastObserved = frame }
local sectorsW, sectorsH = 0, 0
local myAllyTeam = nil

--------------------------------------------------------------------------------
-- Helpers
--------------------------------------------------------------------------------

local function initScoutMap()
    local mapW = Game.mapSizeX
    local mapH = Game.mapSizeZ
    sectorsW = math.ceil(mapW / SECTOR_SIZE)
    sectorsH = math.ceil(mapH / SECTOR_SIZE)

    scoutMap = {}
    for sz = 1, sectorsH do
        scoutMap[sz] = {}
        for sx = 1, sectorsW do
            scoutMap[sz][sx] = { lastObserved = -1 }
        end
    end
end

local function worldToSector(x, z)
    local sx = math.floor(x / SECTOR_SIZE) + 1
    local sz = math.floor(z / SECTOR_SIZE) + 1
    return math.max(1, math.min(sx, sectorsW)), math.max(1, math.min(sz, sectorsH))
end

local function markSectorObserved(x, z, frame)
    local sx, sz = worldToSector(x, z)
    scoutMap[sz][sx].lastObserved = frame
end

--------------------------------------------------------------------------------
-- Tool handlers
--------------------------------------------------------------------------------

local function handleToolCall(toolName, args)
    if toolName == "intel:known_enemies" then
        local currentFrame = Spring.GetGameFrame()
        local results = {}

        for unitID, info in pairs(knownEnemies) do
            local age = currentFrame - info.lastSeen
            local confidence = math.max(0, 1.0 - age * DECAY_RATE)

            -- Skip entries with near-zero confidence
            if confidence > 0.05 then
                results[#results + 1] = string.format(
                    '{"id":%d,"name":"%s","x":%.0f,"z":%.0f,"lastSeen":%d,"confidence":%.2f,"inLos":%s}',
                    unitID, info.name or "unknown",
                    info.x or 0, info.z or 0,
                    info.lastSeen, confidence,
                    info.inLos and "true" or "false"
                )
            end
        end

        return {
            type = "text",
            text = string.format('{"count":%d,"enemies":[%s]}', #results, table.concat(results, ",")),
        }

    elseif toolName == "intel:hud" then
        local currentFrame = Spring.GetGameFrame()
        local totalKnown = 0
        local fresh = 0     -- confidence > 0.7
        local stale = 0     -- confidence <= 0.7

        for unitID, info in pairs(knownEnemies) do
            local age = currentFrame - info.lastSeen
            local confidence = math.max(0, 1.0 - age * DECAY_RATE)
            if confidence > 0.05 then
                totalKnown = totalKnown + 1
                if confidence > 0.7 then
                    fresh = fresh + 1
                else
                    stale = stale + 1
                end
            end
        end

        -- Compute scouted percentage
        local totalSectors = sectorsW * sectorsH
        local scoutedCount = 0
        for sz = 1, sectorsH do
            for sx = 1, sectorsW do
                if scoutMap[sz][sx].lastObserved >= 0 then
                    scoutedCount = scoutedCount + 1
                end
            end
        end
        local scoutedPct = totalSectors > 0 and math.floor(scoutedCount / totalSectors * 100) or 0

        return {
            type = "text",
            text = string.format(
                "Known enemies: %d (%d fresh, %d stale) | Scouted: %d%%",
                totalKnown, fresh, stale, scoutedPct
            ),
        }

    elseif toolName == "intel:desc" then
        return {
            type = "text",
            text = "Intel tracks enemy last-known positions with confidence that decays over time since last sighting. " ..
                   "Fresh contacts (confidence >0.7) were seen recently; stale ones may have moved. " ..
                   "The scouting map divides the map into sectors — staleness=-1 means never scouted, prioritize those areas. " ..
                   "Use intel:known_enemies for detailed enemy positions. Enable the HUD overlay for automatic intel awareness.",
        }

    elseif toolName == "intel:scouted_areas" then
        local currentFrame = Spring.GetGameFrame()
        local sectors = {}

        for sz = 1, sectorsH do
            for sx = 1, sectorsW do
                local cell = scoutMap[sz][sx]
                local staleness
                if cell.lastObserved < 0 then
                    staleness = -1  -- never observed
                else
                    staleness = currentFrame - cell.lastObserved
                end

                sectors[#sectors + 1] = string.format(
                    '{"sx":%d,"sz":%d,"x":%.0f,"z":%.0f,"staleness":%d}',
                    sx, sz,
                    (sx - 0.5) * SECTOR_SIZE,
                    (sz - 0.5) * SECTOR_SIZE,
                    staleness
                )
            end
        end

        return {
            type = "text",
            text = string.format(
                '{"sector_size":%d,"sectors_w":%d,"sectors_h":%d,"sectors":[%s]}',
                SECTOR_SIZE, sectorsW, sectorsH, table.concat(sectors, ",")
            ),
        }
    end

    return { type = "text", text = '{"error":"unknown tool"}' }
end

--------------------------------------------------------------------------------
-- Widget lifecycle
--------------------------------------------------------------------------------

function widget:Initialize()
    myAllyTeam = Spring.GetMyAllyTeamID()
    initScoutMap()

    if WG.AgentTools and WG.AgentTools.Register then
        WG.AgentTools.Register("intel", {
            {
                name = "intel:known_enemies",
                description = "Get last-known enemy positions with confidence scores. Confidence decays over time since last sighting.",
                inputSchema = { type = "object" },
            },
            {
                name = "intel:hud",
                description = "Compact one-line intel summary for HUD overlay.",
                inputSchema = { type = "object" },
            },
            {
                name = "intel:desc",
                description = "Usage guide for intel tools.",
                inputSchema = { type = "object" },
            },
            {
                name = "intel:scouted_areas",
                description = "Get the scouting map showing sector staleness (frames since last observed). Staleness -1 means never scouted.",
                inputSchema = { type = "object" },
            },
        }, handleToolCall)
    else
        Spring.Echo("[AgentIntel] WG.AgentTools not available — tools not registered")
    end
end

--------------------------------------------------------------------------------
-- LOS tracking callins
--------------------------------------------------------------------------------

function widget:UnitEnteredLos(unitID, unitTeam, allyTeam, unitDefID)
    if allyTeam == myAllyTeam then return end  -- ignore own/allied units

    local x, y, z = Spring.GetUnitPosition(unitID)
    local name = "unknown"
    if unitDefID and UnitDefs[unitDefID] then
        name = UnitDefs[unitDefID].name
    end

    knownEnemies[unitID] = {
        defID = unitDefID,
        name = name,
        x = x or 0,
        z = z or 0,
        lastSeen = Spring.GetGameFrame(),
        inLos = true,
    }

    if x then
        markSectorObserved(x, z, Spring.GetGameFrame())
    end
end

function widget:UnitLeftLos(unitID, unitTeam, allyTeam, unitDefID)
    if allyTeam == myAllyTeam then return end

    local entry = knownEnemies[unitID]
    if entry then
        -- Update last known position before leaving LOS
        local x, y, z = Spring.GetUnitPosition(unitID)
        if x then
            entry.x = x
            entry.z = z
        end
        entry.lastSeen = Spring.GetGameFrame()
        entry.inLos = false
    end
end

function widget:UnitDestroyed(unitID, unitDefID, unitTeam)
    -- Remove destroyed enemies from tracking
    knownEnemies[unitID] = nil
end

-- Periodically update positions of enemies still in LOS
function widget:GameFrame(frame)
    if frame % 30 ~= 0 then return end

    for unitID, info in pairs(knownEnemies) do
        if info.inLos then
            local x, y, z = Spring.GetUnitPosition(unitID)
            if x then
                info.x = x
                info.z = z
                info.lastSeen = frame
                markSectorObserved(x, z, frame)
            else
                -- Unit no longer accessible (may have been destroyed)
                info.inLos = false
            end
        end
    end
end
