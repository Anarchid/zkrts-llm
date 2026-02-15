-- Agent Squads Widget
-- Persistent named unit groups. Survives unit death (auto-removes dead units).
-- Provides tools to create, manage, and issue orders to squads.

function widget:GetInfo()
    return {
        name    = "Agent Squads",
        desc    = "Named unit group management for agent coordination",
        author  = "afcomech",
        version = "0.1",
        date    = "2026",
        license = "MIT",
        layer   = 1,
        enabled = true,
    }
end

--------------------------------------------------------------------------------
-- State
--------------------------------------------------------------------------------

local squads = {}          -- id -> { name, units = {unitID -> true}, created }
local nextSquadID = 1

--------------------------------------------------------------------------------
-- Command constants (from Spring)
--------------------------------------------------------------------------------

local CMD_MOVE       = 10
local CMD_PATROL     = 15
local CMD_FIGHT      = 16
local CMD_ATTACK     = 20
local CMD_GUARD      = 25
local CMD_STOP       = 0
local CMD_WAIT       = 5
local CMD_OPT_SHIFT  = 32

local COMMAND_MAP = {
    move   = CMD_MOVE,
    patrol = CMD_PATROL,
    fight  = CMD_FIGHT,
    attack = CMD_ATTACK,
    guard  = CMD_GUARD,
    stop   = CMD_STOP,
    wait   = CMD_WAIT,
}

--------------------------------------------------------------------------------
-- Helpers
--------------------------------------------------------------------------------

local function squadToJSON(id, squad)
    local aliveUnits = {}
    for unitID, _ in pairs(squad.units) do
        if Spring.ValidUnitID(unitID) and not Spring.GetUnitIsDead(unitID) then
            aliveUnits[#aliveUnits + 1] = unitID
        else
            squad.units[unitID] = nil  -- auto-remove dead units
        end
    end

    local unitStrs = {}
    for _, uid in ipairs(aliveUnits) do
        local defID = Spring.GetUnitDefID(uid)
        local name = (defID and UnitDefs[defID]) and UnitDefs[defID].name or "unknown"
        local x, y, z = Spring.GetUnitPosition(uid)
        unitStrs[#unitStrs + 1] = string.format(
            '{"id":%d,"name":"%s","x":%.0f,"z":%.0f}',
            uid, name, x or 0, z or 0
        )
    end

    return string.format(
        '{"id":%d,"name":"%s","size":%d,"units":[%s]}',
        id, squad.name, #aliveUnits, table.concat(unitStrs, ",")
    )
end

local function parseUnitIDs(ids)
    local result = {}
    if type(ids) == "table" then
        for _, v in ipairs(ids) do
            local id = tonumber(v)
            if id then result[#result + 1] = id end
        end
    end
    return result
end

--------------------------------------------------------------------------------
-- Tool handlers
--------------------------------------------------------------------------------

local function handleToolCall(toolName, args)
    if toolName == "squad:create" then
        local name = args.name
        if not name or name == "" then
            return { type = "text", text = '{"error":"missing squad name"}' }
        end

        local unitIDs = parseUnitIDs(args.ids or {})
        local id = nextSquadID
        nextSquadID = nextSquadID + 1

        local units = {}
        for _, uid in ipairs(unitIDs) do
            if Spring.ValidUnitID(uid) then
                units[uid] = true
            end
        end

        squads[id] = {
            name = name,
            units = units,
            created = Spring.GetGameFrame(),
        }

        return {
            type = "text",
            text = squadToJSON(id, squads[id]),
        }

    elseif toolName == "squad:list" then
        local results = {}
        for id, squad in pairs(squads) do
            results[#results + 1] = squadToJSON(id, squad)
        end

        return {
            type = "text",
            text = string.format('{"squads":[%s]}', table.concat(results, ",")),
        }

    elseif toolName == "squad:disband" then
        local id = tonumber(args.id)
        if not id or not squads[id] then
            return { type = "text", text = '{"error":"squad not found"}' }
        end
        local name = squads[id].name
        squads[id] = nil
        return {
            type = "text",
            text = string.format('{"disbanded":true,"name":"%s"}', name),
        }

    elseif toolName == "squad:add" then
        local id = tonumber(args.id)
        if not id or not squads[id] then
            return { type = "text", text = '{"error":"squad not found"}' }
        end
        local unitIDs = parseUnitIDs(args.units or {})
        local added = 0
        for _, uid in ipairs(unitIDs) do
            if Spring.ValidUnitID(uid) then
                squads[id].units[uid] = true
                added = added + 1
            end
        end
        return {
            type = "text",
            text = squadToJSON(id, squads[id]),
        }

    elseif toolName == "squad:remove" then
        local id = tonumber(args.id)
        if not id or not squads[id] then
            return { type = "text", text = '{"error":"squad not found"}' }
        end
        local unitIDs = parseUnitIDs(args.units or {})
        for _, uid in ipairs(unitIDs) do
            squads[id].units[uid] = nil
        end
        return {
            type = "text",
            text = squadToJSON(id, squads[id]),
        }

    elseif toolName == "squad:order" then
        local id = tonumber(args.id)
        if not id or not squads[id] then
            return { type = "text", text = '{"error":"squad not found"}' }
        end

        local command = args.command
        local cmdID = COMMAND_MAP[command]
        if not cmdID then
            return {
                type = "text",
                text = string.format('{"error":"unknown command: %s. Available: move, patrol, fight, attack, guard, stop, wait"}', tostring(command)),
            }
        end

        local params = args.params or {}
        local cmdParams = {}
        local cmdOpts = {}

        if command == "move" or command == "patrol" or command == "fight" then
            cmdParams = { tonumber(params.x) or 0, 0, tonumber(params.z) or 0 }
        elseif command == "attack" then
            cmdParams = { tonumber(params.target_id) or 0 }
        elseif command == "guard" then
            cmdParams = { tonumber(params.guard_id) or 0 }
        end

        if params.queue then
            cmdOpts = { "shift" }
        end

        local ordered = 0
        for unitID, _ in pairs(squads[id].units) do
            if Spring.ValidUnitID(unitID) and not Spring.GetUnitIsDead(unitID) then
                Spring.GiveOrderToUnit(unitID, cmdID, cmdParams, cmdOpts)
                ordered = ordered + 1
            end
        end

        return {
            type = "text",
            text = string.format(
                '{"ordered":%d,"command":"%s","squad":"%s"}',
                ordered, command, squads[id].name
            ),
        }
    end

    return { type = "text", text = '{"error":"unknown tool"}' }
end

--------------------------------------------------------------------------------
-- Widget lifecycle
--------------------------------------------------------------------------------

function widget:Initialize()
    if WG.AgentTools and WG.AgentTools.Register then
        WG.AgentTools.Register("squads", {
            {
                name = "squad:create",
                description = "Create a named squad with initial unit IDs.",
                inputSchema = {
                    type = "object",
                    properties = {
                        name = { type = "string", description = "Squad name" },
                        ids = {
                            type = "array",
                            items = { type = "number" },
                            description = "Initial unit IDs to add",
                        },
                    },
                    required = { "name" },
                },
            },
            {
                name = "squad:list",
                description = "List all squads with their units.",
                inputSchema = { type = "object" },
            },
            {
                name = "squad:disband",
                description = "Disband (delete) a squad.",
                inputSchema = {
                    type = "object",
                    properties = {
                        id = { type = "number", description = "Squad ID" },
                    },
                    required = { "id" },
                },
            },
            {
                name = "squad:add",
                description = "Add units to an existing squad.",
                inputSchema = {
                    type = "object",
                    properties = {
                        id = { type = "number", description = "Squad ID" },
                        units = {
                            type = "array",
                            items = { type = "number" },
                            description = "Unit IDs to add",
                        },
                    },
                    required = { "id", "units" },
                },
            },
            {
                name = "squad:remove",
                description = "Remove units from a squad.",
                inputSchema = {
                    type = "object",
                    properties = {
                        id = { type = "number", description = "Squad ID" },
                        units = {
                            type = "array",
                            items = { type = "number" },
                            description = "Unit IDs to remove",
                        },
                    },
                    required = { "id", "units" },
                },
            },
            {
                name = "squad:order",
                description = "Issue a command to all units in a squad. Commands: move, patrol, fight, attack, guard, stop, wait.",
                inputSchema = {
                    type = "object",
                    properties = {
                        id = { type = "number", description = "Squad ID" },
                        command = { type = "string", description = "Command name (move/patrol/fight/attack/guard/stop/wait)" },
                        params = {
                            type = "object",
                            description = "Command parameters. For move/patrol/fight: {x, z}. For attack: {target_id}. For guard: {guard_id}. Optional: {queue: true} to shift-queue.",
                        },
                    },
                    required = { "id", "command" },
                },
            },
        }, handleToolCall)
    else
        Spring.Echo("[AgentSquads] WG.AgentTools not available — tools not registered")
    end
end

-- Auto-remove dead units from all squads
function widget:UnitDestroyed(unitID, unitDefID, unitTeam)
    for _, squad in pairs(squads) do
        squad.units[unitID] = nil
    end
end
