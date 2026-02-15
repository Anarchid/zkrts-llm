-- Agent Economy Widget
-- Tracks metal/energy resources and maintains a history ring buffer.
-- Queries Spring.GetTeamResources() each update.

function widget:GetInfo()
    return {
        name    = "Agent Economy",
        desc    = "Economy tracking and history for agent decision-making",
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

local UPDATE_RATE   = 30    -- frames between samples
local HISTORY_SIZE  = 120   -- ring buffer size (~120s at 30fps/30frame rate)

--------------------------------------------------------------------------------
-- State
--------------------------------------------------------------------------------

local myTeamID = nil
local history = {}          -- ring buffer of snapshots
local historyIdx = 0
local historyCount = 0

-- Resource IDs in Spring
local RES_METAL  = 0
local RES_ENERGY = 1

--------------------------------------------------------------------------------
-- Helpers
--------------------------------------------------------------------------------

local function getSnapshot()
    if not myTeamID then return nil end

    local mCur, mStor, mPull, mInc, mExp, mShare, mSent, mRec =
        Spring.GetTeamResources(myTeamID, "metal")
    local eCur, eStor, ePull, eInc, eExp, eShare, eSent, eRec =
        Spring.GetTeamResources(myTeamID, "energy")

    if not mCur then return nil end

    local metalStallRisk = (mInc > 0) and (mCur / mInc < 2) or false
    local energyStallRisk = (eInc > 0) and (eCur / eInc < 2) or false

    return {
        frame = Spring.GetGameFrame(),
        metal = {
            current = mCur,
            storage = mStor,
            income  = mInc,
            expense = mExp,
            pull    = mPull,
        },
        energy = {
            current = eCur,
            storage = eStor,
            income  = eInc,
            expense = eExp,
            pull    = ePull,
        },
        stall_risk = {
            metal  = metalStallRisk,
            energy = energyStallRisk,
        },
    }
end

local function addToHistory(snap)
    historyIdx = (historyIdx % HISTORY_SIZE) + 1
    history[historyIdx] = snap
    if historyCount < HISTORY_SIZE then
        historyCount = historyCount + 1
    end
end

local function formatNumber(n)
    return string.format("%.1f", n or 0)
end

--------------------------------------------------------------------------------
-- Tool handlers
--------------------------------------------------------------------------------

local function handleToolCall(toolName, args)
    if toolName == "economy:snapshot" then
        local snap = getSnapshot()
        if not snap then
            return { type = "text", text = '{"error":"no team data available"}' }
        end

        local text = string.format(
            '{"frame":%d,' ..
            '"metal":{"current":%s,"storage":%s,"income":%s,"expense":%s},' ..
            '"energy":{"current":%s,"storage":%s,"income":%s,"expense":%s},' ..
            '"stall_risk":{"metal":%s,"energy":%s}}',
            snap.frame,
            formatNumber(snap.metal.current), formatNumber(snap.metal.storage),
            formatNumber(snap.metal.income), formatNumber(snap.metal.expense),
            formatNumber(snap.energy.current), formatNumber(snap.energy.storage),
            formatNumber(snap.energy.income), formatNumber(snap.energy.expense),
            tostring(snap.stall_risk.metal), tostring(snap.stall_risk.energy)
        )

        return { type = "text", text = text }

    elseif toolName == "economy:history" then
        local maxFrames = tonumber(args.frames) or 60
        local maxSamples = math.min(maxFrames, historyCount)

        local samples = {}
        for i = 1, maxSamples do
            local idx = ((historyIdx - i) % HISTORY_SIZE) + 1
            local snap = history[idx]
            if snap then
                samples[#samples + 1] = string.format(
                    '{"frame":%d,"m_inc":%s,"m_exp":%s,"m_cur":%s,"e_inc":%s,"e_exp":%s,"e_cur":%s}',
                    snap.frame,
                    formatNumber(snap.metal.income), formatNumber(snap.metal.expense),
                    formatNumber(snap.metal.current),
                    formatNumber(snap.energy.income), formatNumber(snap.energy.expense),
                    formatNumber(snap.energy.current)
                )
            end
        end

        return {
            type = "text",
            text = string.format('{"samples":[%s],"count":%d}', table.concat(samples, ","), #samples),
        }
    end

    return { type = "text", text = '{"error":"unknown tool"}' }
end

--------------------------------------------------------------------------------
-- Widget lifecycle
--------------------------------------------------------------------------------

function widget:Initialize()
    myTeamID = Spring.GetMyTeamID()

    if WG.AgentTools and WG.AgentTools.Register then
        WG.AgentTools.Register("economy", {
            {
                name = "economy:snapshot",
                description = "Get current economy state: metal/energy current, storage, income, expense, and stall risk indicators.",
                inputSchema = { type = "object" },
            },
            {
                name = "economy:history",
                description = "Get economy history time series (sampled every ~1s).",
                inputSchema = {
                    type = "object",
                    properties = {
                        frames = { type = "number", description = "Number of samples to return (default: 60, max: 120)" },
                    },
                },
            },
        }, handleToolCall)
    else
        Spring.Echo("[AgentEconomy] WG.AgentTools not available — tools not registered")
    end
end

function widget:GameFrame(frame)
    if frame % UPDATE_RATE == 0 then
        local snap = getSnapshot()
        if snap then
            addToHistory(snap)
        end
    end
end
