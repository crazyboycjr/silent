--- @since 26.1.22
-- Previews in silent, as Space does in Nautilus: one key shows the hovered
-- file, the preview follows yazi's cursor while it is open, and the keyboard
-- stays in yazi. silent is started with yazi and stays while yazi runs.

local M = {}

-- The file itself: in search results the url is a search:// one.
local function hovered()
	local h = cx.active.current.hovered
	return h and tostring(h.path) or nil
end

-- silent from PATH, else where an install of a user's own puts it.
local function run(state, args)
	local bins = state.bin and { state.bin } or { "silent", os.getenv("HOME") .. "/.local/bin/silent" }
	for _, bin in ipairs(bins) do
		local out = Command(bin):arg(args):stdout(Command.PIPED):stderr(Command.NULL):output()
		if out then
			state.bin = bin
			return out
		end
	end
end

-- Runs `silent VERB [FILE]` one at a time; while one runs, the newest file
-- to follow replaces any older one still waiting. silent says whether the
-- preview is open, which also catches it being closed with Esc there.
local function ask(state, verb, file)
	state.last = file
	local queue = state.queue or {}
	if verb == "follow" and #queue > 0 and queue[#queue][1] == "follow" then
		queue[#queue] = { verb, file }
	else
		queue[#queue + 1] = { verb, file }
	end
	state.queue = queue
	if state.busy then
		return
	end
	state.busy = true
	ya.async(function()
		while #state.queue > 0 do
			local out = run(state, table.remove(state.queue, 1))
			state.open = out ~= nil and out.status.success and out.stdout:find("shown", 1, true) ~= nil
		end
		state.busy = false
	end)
end

local toggle = ya.sync(function(state)
	local file = hovered()
	if file then
		ask(state, "toggle", file)
	end
end)

-- opts.bin: the silent to run, if neither PATH nor ~/.local/bin has it.
function M:setup(opts)
	local state = self
	state.bin = opts and opts.bin
	ask(state, "warm")
	-- yazi says hover more than once for the same file.
	ps.sub("hover", function()
		local file = hovered()
		if state.open and file and file ~= state.last then
			ask(state, "follow", file)
		end
	end)
end

function M:entry() toggle() end

return M
