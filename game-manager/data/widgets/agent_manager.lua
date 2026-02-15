-- Agent Manager Widget
-- Routes tool calls from SAI bridge to child agent widgets.
-- Child widgets register via WG.AgentTools.Register() and receive calls synchronously.
--
-- Wire protocol (JSON over Spring.SendSkirmishAIMessage / RecvSkirmishAIMessage):
--   SAI -> Widget: {"op":"call","call_id":"...","tool":"name","args":{...}}
--   Widget -> SAI: {"op":"result","call_id":"...","content":[...],"is_error":false}
--   Widget -> SAI: {"op":"register","source":"widget_name","tools":[...]}

local JSON -- loaded in Initialize

function widget:GetInfo()
    return {
        name    = "Agent Manager",
        desc    = "Routes agent tool calls to child widgets",
        author  = "afcomech",
        version = "0.1",
        date    = "2026",
        license = "MIT",
        layer   = -1,  -- load before child widgets
        enabled = true,
    }
end

-- Shared table for child widget registration (WG = Widget Globals, shared across widgets)
WG.AgentTools = WG.AgentTools or {}

-- Internal state
local handlers = {}     -- tool_name -> { handler=fn, source=string }
local aiTeamID = nil    -- the AI team we communicate with

--------------------------------------------------------------------------------
-- JSON helpers (use Spring's built-in or a minimal fallback)
--------------------------------------------------------------------------------

local function initJSON()
    -- Spring includes a JSON library accessible via VFS
    if Spring.Utilities and Spring.Utilities.json then
        JSON = Spring.Utilities.json
        return
    end
    -- Try loading from VFS
    local ok, lib = pcall(function()
        return VFS.Include("LuaUI/Utilities/json.lua", nil, VFS.RAW_FIRST)
    end)
    if ok and lib then
        JSON = lib
        return
    end
    -- Minimal inline JSON encoder/decoder (subset sufficient for tool protocol)
    JSON = {}

    function JSON.encode(val)
        if val == nil then return "null" end
        local t = type(val)
        if t == "string" then
            return '"' .. val:gsub('\\', '\\\\'):gsub('"', '\\"'):gsub('\n', '\\n'):gsub('\r', '\\r'):gsub('\t', '\\t') .. '"'
        elseif t == "number" then
            return tostring(val)
        elseif t == "boolean" then
            return val and "true" or "false"
        elseif t == "table" then
            -- Check if array
            local isArr = (#val > 0) or next(val) == nil
            if isArr and #val > 0 then
                local parts = {}
                for i = 1, #val do
                    parts[i] = JSON.encode(val[i])
                end
                return "[" .. table.concat(parts, ",") .. "]"
            else
                local parts = {}
                for k, v in pairs(val) do
                    parts[#parts + 1] = JSON.encode(tostring(k)) .. ":" .. JSON.encode(v)
                end
                return "{" .. table.concat(parts, ",") .. "}"
            end
        end
        return "null"
    end

    -- Minimal JSON decode (handles the subset we need)
    function JSON.decode(str)
        -- Use Spring's loadstring-based approach as fallback
        -- This is safe because we only decode messages from our own SAI bridge
        local ok, result = pcall(function()
            -- Convert JSON to Lua table literal
            local lua_str = str
                :gsub('null', 'nil')
                :gsub('%[', '{')
                :gsub('%]', '}')
                :gsub('"([^"]-)":', '["%1"]=')
            return assert(load("return " .. lua_str))()
        end)
        if ok then return result end
        -- Last resort: return raw string wrapped
        return nil
    end
end

--------------------------------------------------------------------------------
-- AgentTools API (called by child widgets)
--------------------------------------------------------------------------------

--- Register tools from a child widget.
--- @param source string  Widget name (e.g. "threatmap")
--- @param tools table    Array of {name=, description=, inputSchema=}
--- @param handler function(toolName, args) -> {type="text", text=...} or array of content blocks
function WG.AgentTools.Register(source, tools, handler)
    if not handler then
        Spring.Echo("[AgentManager] ERROR: Register called without handler for " .. tostring(source))
        return
    end

    local toolDefs = {}
    for _, tool in ipairs(tools) do
        handlers[tool.name] = { handler = handler, source = source }
        toolDefs[#toolDefs + 1] = {
            name = tool.name,
            description = tool.description,
            inputSchema = tool.inputSchema or { type = "object" },
        }
        Spring.Echo("[AgentManager] Registered tool: " .. tool.name .. " (from " .. source .. ")")
    end

    -- Notify the SAI bridge about new tools
    if aiTeamID then
        local msg = JSON.encode({
            op = "register",
            source = source,
            tools = toolDefs,
        })
        Spring.SendSkirmishAIMessage(aiTeamID, msg)
    end
end

--- Unregister tools by name.
--- @param toolNames table  Array of tool name strings
function WG.AgentTools.Unregister(toolNames)
    local removed = {}
    for _, name in ipairs(toolNames) do
        if handlers[name] then
            handlers[name] = nil
            removed[#removed + 1] = name
            Spring.Echo("[AgentManager] Unregistered tool: " .. name)
        end
    end

    if #removed > 0 and aiTeamID then
        local msg = JSON.encode({
            op = "unregister",
            tools = removed,
        })
        Spring.SendSkirmishAIMessage(aiTeamID, msg)
    end
end

--------------------------------------------------------------------------------
-- Widget lifecycle
--------------------------------------------------------------------------------

function widget:Initialize()
    initJSON()
    Spring.Echo("[AgentManager] Initialized")
end

--- Find the AgentBridge AI team ID.
local function findAiTeam()
    local teams = Spring.GetTeamList()
    for _, teamID in ipairs(teams) do
        local _, leader, isDead, isAI, side, allyTeam, _, shortName = Spring.GetTeamInfo(teamID)
        if isAI and shortName and shortName:find("AgentBridge") then
            return teamID
        end
    end
    return nil
end

function widget:GamePreload()
    aiTeamID = findAiTeam()
    if aiTeamID then
        Spring.Echo("[AgentManager] Found AgentBridge AI team: " .. tostring(aiTeamID))
    else
        Spring.Echo("[AgentManager] AgentBridge AI not found yet (will retry at GameStart)")
    end
end

function widget:GameStart()
    if not aiTeamID then
        aiTeamID = findAiTeam()
        if aiTeamID then
            Spring.Echo("[AgentManager] Found AgentBridge AI team at GameStart: " .. tostring(aiTeamID))
        else
            Spring.Echo("[AgentManager] WARNING: AgentBridge AI team not found")
        end
    end
end

--------------------------------------------------------------------------------
-- Message handling
--------------------------------------------------------------------------------

--- Receive a message from the SAI bridge and return a response synchronously.
function widget:RecvSkirmishAIMessage(aiTeam, dataStr)
    if not dataStr or dataStr == "" then return "" end

    local ok, msg = pcall(JSON.decode, dataStr)
    if not ok or not msg then
        Spring.Echo("[AgentManager] Failed to parse message: " .. tostring(dataStr))
        return ""
    end

    local op = msg.op

    if op == "call" then
        local call_id = msg.call_id or ""
        local toolName = msg.tool or ""
        local args = msg.args or {}

        local entry = handlers[toolName]
        if not entry then
            return JSON.encode({
                op = "result",
                call_id = call_id,
                content = {{ type = "text", text = "Unknown tool: " .. toolName }},
                is_error = true,
            })
        end

        -- Call the handler (synchronous)
        local success, result = pcall(entry.handler, toolName, args)
        if not success then
            return JSON.encode({
                op = "result",
                call_id = call_id,
                content = {{ type = "text", text = "Tool handler error: " .. tostring(result) }},
                is_error = true,
            })
        end

        -- Normalize result to content array
        local content
        if type(result) == "string" then
            content = {{ type = "text", text = result }}
        elseif type(result) == "table" then
            if result.type then
                -- Single content block
                content = { result }
            else
                -- Array of content blocks (or already correct format)
                content = result
            end
        else
            content = {{ type = "text", text = tostring(result) }}
        end

        return JSON.encode({
            op = "result",
            call_id = call_id,
            content = content,
            is_error = false,
        })
    elseif op == "ping" then
        return JSON.encode({ op = "pong" })
    end

    return ""
end
