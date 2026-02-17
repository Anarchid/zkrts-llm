-- Agent Roster Widget
-- Tracks own units via UnitCreated/Destroyed callins, grouped by role category.
-- Also provides visible enemy roster.

function widget:GetInfo()
    return {
        name    = "Agent Roster",
        desc    = "Unit roster tracking by role for agent decision-making",
        author  = "afcomech",
        version = "0.1",
        date    = "2026",
        license = "MIT",
        layer   = 1,
        enabled = true,
    }
end

--------------------------------------------------------------------------------
-- Role classification
--------------------------------------------------------------------------------

-- Map unit def names to categories (based on Zero-K's CAI classification)
local ROLE_MAP = {
    -- Factories
    cloakfac    = "factory", shieldfac   = "factory", jumpfac     = "factory",
    tankfac     = "factory", hoverfac    = "factory", spiderfac   = "factory",
    amphfac     = "factory", gunshipfac  = "factory", planefac    = "factory",
    shipfac     = "factory", striderarty = "factory",

    -- Raiders
    cloakraid   = "raider", shieldraid   = "raider", jumpraid     = "raider",
    vehraid     = "raider", hoverraid    = "raider", spiderraid   = "raider",
    amphraid    = "raider", gunshipraid  = "raider",

    -- Assault
    cloakassault = "assault", shieldassault = "assault", jumpassault = "assault",
    tankassault  = "assault", hoverassault  = "assault", spiderassault = "assault",
    amphriot     = "assault",

    -- Skirmishers
    cloakskirm   = "skirm", shieldskirm   = "skirm", jumpskirm     = "skirm",
    tankheavyassault = "skirm", hoverskirm = "skirm",

    -- Riot
    cloakriot    = "riot", shieldriot    = "riot",

    -- Artillery
    cloakarty    = "arty", shieldarty    = "arty", jumparty      = "arty",
    tankarty     = "arty",

    -- Anti-air
    cloakaa      = "aa", shieldaa      = "aa", jumpaa        = "aa",
    vehaa        = "aa", hoveraa       = "aa", gunshipaa     = "aa",

    -- Constructors
    cloakcon     = "con", shieldcon     = "con", jumpcon       = "con",
    tankcon      = "con", hovercon      = "con", spidercon     = "con",
    amphcon      = "con", gunshipcon    = "con", planecon      = "con",

    -- Commanders
    dyntrainer_strike_base = "commander",
    dyntrainer_recon_base  = "commander",
    dyntrainer_support_base = "commander",
    dyntrainer_assault_base = "commander",
}

local function classifyUnit(defID)
    if not defID or not UnitDefs[defID] then return "other" end
    local name = UnitDefs[defID].name
    return ROLE_MAP[name] or "other"
end

--------------------------------------------------------------------------------
-- State
--------------------------------------------------------------------------------

local myTeamID = nil
local ownUnits = {}   -- unitID -> { defID, role, name }

--------------------------------------------------------------------------------
-- Helpers
--------------------------------------------------------------------------------

local function getUnitInfo(unitID)
    local defID = Spring.GetUnitDefID(unitID)
    if not defID then return nil end
    local def = UnitDefs[defID]
    if not def then return nil end
    local hp, maxHp, _, _, buildProgress = Spring.GetUnitHealth(unitID)
    local x, y, z = Spring.GetUnitPosition(unitID)
    return {
        id = unitID,
        defID = defID,
        name = def.name,
        humanName = def.humanName,
        role = classifyUnit(defID),
        hp = hp,
        maxHp = maxHp,
        buildProgress = buildProgress or 1.0,
        x = x, y = y, z = z,
        metalCost = def.metalCost,
    }
end

local function formatUnit(info)
    local base = string.format(
        '{"id":%d,"name":"%s","role":"%s","hp":%.0f,"maxHp":%.0f,"x":%.0f,"z":%.0f',
        info.id, info.name, info.role,
        info.hp or 0, info.maxHp or 0,
        info.x or 0, info.z or 0
    )
    if info.buildProgress < 1.0 then
        base = base .. string.format(',"building":true,"buildPct":%.0f', info.buildProgress * 100)
    end
    return base .. "}"
end

--------------------------------------------------------------------------------
-- Tool handlers
--------------------------------------------------------------------------------

local function handleToolCall(toolName, args)
    if toolName == "roster:own" then
        -- Group units by role
        local groups = {}
        local totalCount = 0
        for unitID, entry in pairs(ownUnits) do
            local info = getUnitInfo(unitID)
            if info then
                local role = info.role
                if not groups[role] then
                    groups[role] = {}
                end
                groups[role][#groups[role] + 1] = formatUnit(info)
                totalCount = totalCount + 1
            end
        end

        local parts = {}
        for role, units in pairs(groups) do
            parts[#parts + 1] = string.format('"%s":[%s]', role, table.concat(units, ","))
        end

        return {
            type = "text",
            text = string.format('{"total":%d,%s}', totalCount, table.concat(parts, ",")),
        }

    elseif toolName == "roster:hud" then
        -- Compact summary: count by role, separate building units
        local counts = {}
        local total = 0
        local building = 0
        for unitID, entry in pairs(ownUnits) do
            local info = getUnitInfo(unitID)
            if info then
                local role = info.role
                counts[role] = (counts[role] or 0) + 1
                total = total + 1
                if info.buildProgress < 1.0 then
                    building = building + 1
                end
            end
        end
        local parts = {}
        -- Fixed order for consistent output
        local roleOrder = {"commander", "factory", "con", "raider", "assault", "skirm", "riot", "arty", "aa", "other"}
        for _, role in ipairs(roleOrder) do
            if counts[role] then
                local abbrev = role:sub(1, 4)
                parts[#parts + 1] = string.format("%s×%d", abbrev, counts[role])
            end
        end

        -- Count visible enemies
        local myAllyTeam = Spring.GetMyAllyTeamID()
        local enemies = Spring.GetVisibleUnits(-1, nil, false)
        local enemyCount = 0
        if enemies then
            for _, unitID in ipairs(enemies) do
                if Spring.GetUnitAllyTeam(unitID) ~= myAllyTeam then
                    enemyCount = enemyCount + 1
                end
            end
        end

        local header
        if building > 0 then
            header = string.format("Own(%d, %d building)", total, building)
        else
            header = string.format("Own(%d)", total)
        end

        return {
            type = "text",
            text = string.format("%s: %s | Enemies visible: %d", header, table.concat(parts, " "), enemyCount),
        }

    elseif toolName == "roster:desc" then
        return {
            type = "text",
            text = "Roster tracks your own units grouped by role (factory, raider, assault, skirm, riot, arty, aa, con, commander, other). " ..
                   "Units marked 'building' are still under construction — don't confuse them with damaged units. " ..
                   "Enemy roster only shows currently visible enemies — not a complete picture. " ..
                   "Use roster:unit for detailed info on a specific unit including its command queue. " ..
                   "Enable the HUD overlay for automatic roster awareness every inference cycle.",
        }

    elseif toolName == "roster:enemies" then
        local myAllyTeam = Spring.GetMyAllyTeamID()
        local enemies = Spring.GetVisibleUnits(-1, nil, false)
        local result = {}

        if enemies then
            for _, unitID in ipairs(enemies) do
                local allyTeam = Spring.GetUnitAllyTeam(unitID)
                if allyTeam ~= myAllyTeam then
                    local info = getUnitInfo(unitID)
                    if info then
                        result[#result + 1] = formatUnit(info)
                    end
                end
            end
        end

        return {
            type = "text",
            text = string.format('{"count":%d,"enemies":[%s]}', #result, table.concat(result, ",")),
        }

    elseif toolName == "roster:unit" then
        local unitID = tonumber(args.id)
        if not unitID then
            return { type = "text", text = '{"error":"missing unit id"}' }
        end

        local info = getUnitInfo(unitID)
        if not info then
            return { type = "text", text = '{"error":"unit not found or not visible"}' }
        end

        local def = UnitDefs[info.defID]
        local commands = Spring.GetUnitCommands(unitID, 3) or {}
        local cmdStrs = {}
        for _, cmd in ipairs(commands) do
            cmdStrs[#cmdStrs + 1] = string.format('{"id":%d}', cmd.id or 0)
        end

        local buildStr = ""
        if info.buildProgress < 1.0 then
            buildStr = string.format(',"building":true,"buildPct":%.0f', info.buildProgress * 100)
        end

        return {
            type = "text",
            text = string.format(
                '{"id":%d,"name":"%s","humanName":"%s","role":"%s",' ..
                '"hp":%.0f,"maxHp":%.0f,"x":%.0f,"y":%.0f,"z":%.0f,' ..
                '"metalCost":%.0f%s,"commands":[%s]}',
                info.id, info.name, info.humanName or "", info.role,
                info.hp or 0, info.maxHp or 0,
                info.x or 0, info.y or 0, info.z or 0,
                info.metalCost or 0,
                buildStr,
                table.concat(cmdStrs, ",")
            ),
        }
    end

    return { type = "text", text = '{"error":"unknown tool"}' }
end

--------------------------------------------------------------------------------
-- Widget lifecycle
--------------------------------------------------------------------------------

function widget:Initialize()
    myTeamID = Spring.GetMyTeamID()

    -- Populate initial roster
    local units = Spring.GetTeamUnits(myTeamID)
    if units then
        for _, unitID in ipairs(units) do
            local defID = Spring.GetUnitDefID(unitID)
            if defID then
                ownUnits[unitID] = {
                    defID = defID,
                    role = classifyUnit(defID),
                    name = UnitDefs[defID] and UnitDefs[defID].name or "unknown",
                }
            end
        end
    end

    if WG.AgentTools and WG.AgentTools.Register then
        WG.AgentTools.Register("roster", {
            {
                name = "roster:own",
                description = "Get own unit roster grouped by role (factory, raider, assault, skirm, riot, arty, aa, con, commander, other). Includes unit positions and health.",
                inputSchema = { type = "object" },
            },
            {
                name = "roster:hud",
                description = "Compact one-line roster summary for HUD overlay.",
                inputSchema = { type = "object" },
            },
            {
                name = "roster:desc",
                description = "Usage guide for roster tools.",
                inputSchema = { type = "object" },
            },
            {
                name = "roster:enemies",
                description = "Get currently visible enemy units with positions and types.",
                inputSchema = { type = "object" },
            },
            {
                name = "roster:unit",
                description = "Get detailed info about a specific unit by ID, including commands queue.",
                inputSchema = {
                    type = "object",
                    properties = {
                        id = { type = "number", description = "Unit ID" },
                    },
                    required = { "id" },
                },
            },
        }, handleToolCall)
    else
        Spring.Echo("[AgentRoster] WG.AgentTools not available — tools not registered")
    end
end

function widget:UnitCreated(unitID, unitDefID, unitTeam)
    if unitTeam == myTeamID then
        ownUnits[unitID] = {
            defID = unitDefID,
            role = classifyUnit(unitDefID),
            name = UnitDefs[unitDefID] and UnitDefs[unitDefID].name or "unknown",
        }
    end
end

function widget:UnitDestroyed(unitID, unitDefID, unitTeam)
    ownUnits[unitID] = nil
end

function widget:UnitGiven(unitID, unitDefID, newTeam, oldTeam)
    if newTeam == myTeamID then
        ownUnits[unitID] = {
            defID = unitDefID,
            role = classifyUnit(unitDefID),
            name = UnitDefs[unitDefID] and UnitDefs[unitDefID].name or "unknown",
        }
    elseif oldTeam == myTeamID then
        ownUnits[unitID] = nil
    end
end

function widget:UnitTaken(unitID, unitDefID, newTeam, oldTeam)
    if oldTeam == myTeamID then
        ownUnits[unitID] = nil
    end
end
