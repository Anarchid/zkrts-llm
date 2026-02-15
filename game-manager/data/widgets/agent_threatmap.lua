-- Agent Threatmap Widget
-- Grid-based threat analysis: tracks enemy positions and computes threat per sector.
-- Updates every 30 frames via widget:GameFrame().

local JSON  -- set by Agent Manager (or loaded locally)

function widget:GetInfo()
    return {
        name    = "Agent Threatmap",
        desc    = "Grid-based threat analysis for agent decision-making",
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

local GRID_SIZE   = 512    -- sector size in world units
local UPDATE_RATE = 30     -- frames between updates

--------------------------------------------------------------------------------
-- State
--------------------------------------------------------------------------------

local gridW, gridH = 0, 0
local grid = {}            -- [gz][gx] = { threat=, units={} }
local mapWidth, mapHeight = 0, 0

--------------------------------------------------------------------------------
-- Helpers
--------------------------------------------------------------------------------

local function worldToGrid(x, z)
    local gx = math.floor(x / GRID_SIZE) + 1
    local gz = math.floor(z / GRID_SIZE) + 1
    return math.max(1, math.min(gx, gridW)), math.max(1, math.min(gz, gridH))
end

local function initGrid()
    mapWidth  = Game.mapSizeX
    mapHeight = Game.mapSizeZ
    gridW = math.ceil(mapWidth / GRID_SIZE)
    gridH = math.ceil(mapHeight / GRID_SIZE)

    grid = {}
    for gz = 1, gridH do
        grid[gz] = {}
        for gx = 1, gridW do
            grid[gz][gx] = { threat = 0, units = {} }
        end
    end
end

local function clearGrid()
    for gz = 1, gridH do
        for gx = 1, gridW do
            grid[gz][gx].threat = 0
            grid[gz][gx].units = {}
        end
    end
end

local function updateThreat()
    clearGrid()

    local myAllyTeam = Spring.GetMyAllyTeamID()
    -- Get all visible enemy units
    local enemies = Spring.GetVisibleUnits(-1, nil, false)
    if not enemies then return end

    for _, unitID in ipairs(enemies) do
        local teamID = Spring.GetUnitTeam(unitID)
        if teamID then
            local allyTeam = Spring.GetUnitAllyTeam(unitID)
            if allyTeam ~= myAllyTeam then
                local x, y, z = Spring.GetUnitPosition(unitID)
                if x then
                    local defID = Spring.GetUnitDefID(unitID)
                    local threat = 1
                    if defID and UnitDefs[defID] then
                        local def = UnitDefs[defID]
                        -- Use metal cost as rough threat proxy
                        threat = (def.metalCost or 50) / 50
                    end
                    local gx, gz_idx = worldToGrid(x, z)
                    local cell = grid[gz_idx][gx]
                    cell.threat = cell.threat + threat
                    cell.units[#cell.units + 1] = {
                        id = unitID,
                        defID = defID,
                        x = x, y = y, z = z,
                        threat = threat,
                    }
                end
            end
        end
    end
end

--------------------------------------------------------------------------------
-- Tool handlers
--------------------------------------------------------------------------------

local function handleToolCall(toolName, args)
    if toolName == "threat:query" then
        local qx = tonumber(args.x) or 0
        local qz = tonumber(args.z) or 0
        local radius = tonumber(args.radius) or GRID_SIZE

        -- Sum threat in cells within radius
        local totalThreat = 0
        local unitBreakdown = {}
        local gridRadius = math.ceil(radius / GRID_SIZE)
        local cx, cz = worldToGrid(qx, qz)

        for dz = -gridRadius, gridRadius do
            for dx = -gridRadius, gridRadius do
                local gx = cx + dx
                local gz = cz + dz
                if gx >= 1 and gx <= gridW and gz >= 1 and gz <= gridH then
                    local cell = grid[gz][gx]
                    totalThreat = totalThreat + cell.threat
                    for _, u in ipairs(cell.units) do
                        local name = "unknown"
                        if u.defID and UnitDefs[u.defID] then
                            name = UnitDefs[u.defID].name
                        end
                        unitBreakdown[#unitBreakdown + 1] = {
                            id = u.id,
                            name = name,
                            threat = u.threat,
                            x = u.x, z = u.z,
                        }
                    end
                end
            end
        end

        return {
            type = "text",
            text = Spring.Utilities and Spring.Utilities.json
                and Spring.Utilities.json.encode({
                    threat = totalThreat,
                    center = { x = qx, z = qz },
                    radius = radius,
                    units = unitBreakdown,
                })
                or string.format(
                    '{"threat":%.1f,"center":{"x":%.0f,"z":%.0f},"radius":%.0f,"unit_count":%d}',
                    totalThreat, qx, qz, radius, #unitBreakdown
                ),
        }

    elseif toolName == "threat:sectors" then
        local sectors = {}
        for gz = 1, gridH do
            for gx = 1, gridW do
                local cell = grid[gz][gx]
                if cell.threat > 0 then
                    sectors[#sectors + 1] = {
                        gx = gx, gz = gz,
                        x = (gx - 0.5) * GRID_SIZE,
                        z = (gz - 0.5) * GRID_SIZE,
                        threat = cell.threat,
                        unit_count = #cell.units,
                    }
                end
            end
        end

        return {
            type = "text",
            text = Spring.Utilities and Spring.Utilities.json
                and Spring.Utilities.json.encode({
                    grid_size = GRID_SIZE,
                    grid_w = gridW,
                    grid_h = gridH,
                    sectors = sectors,
                })
                or string.format(
                    '{"grid_size":%d,"grid_w":%d,"grid_h":%d,"active_sectors":%d}',
                    GRID_SIZE, gridW, gridH, #sectors
                ),
        }
    end

    return { type = "text", text = '{"error":"unknown tool"}' }
end

--------------------------------------------------------------------------------
-- Widget lifecycle
--------------------------------------------------------------------------------

function widget:Initialize()
    initGrid()

    if WG.AgentTools and WG.AgentTools.Register then
        WG.AgentTools.Register("threatmap", {
            {
                name = "threat:query",
                description = "Query threat level at a position. Returns threat score and enemy unit breakdown.",
                inputSchema = {
                    type = "object",
                    properties = {
                        x = { type = "number", description = "World X coordinate" },
                        z = { type = "number", description = "World Z coordinate" },
                        radius = { type = "number", description = "Search radius in world units (default: 512)" },
                    },
                    required = { "x", "z" },
                },
            },
            {
                name = "threat:sectors",
                description = "Get the full threat grid. Returns all sectors with non-zero threat.",
                inputSchema = { type = "object" },
            },
        }, handleToolCall)
    else
        Spring.Echo("[AgentThreatmap] WG.AgentTools not available — tools not registered")
    end
end

function widget:GameFrame(frame)
    if frame % UPDATE_RATE == 0 then
        updateThreat()
    end
end
