-- Bulletproof ASCII playback controls for mpv on headless Pi
-- Left visual half: Play/Pause  |  Right visual half: Exit
-- Auto-hide is DISABLED for debugging

local mp = require 'mp'
local msg = require 'mp.msg'

local overlay = mp.create_osd_overlay("ass-events")

local function get_rotation()
    return mp.get_property_number("video-rotate", 0)
end

local function logical_to_physical(lx, ly, rot)
    if rot == 90 then return 1 - ly, lx
    elseif rot == 180 then return 1 - lx, 1 - ly
    elseif rot == 270 then return ly, 1 - lx
    else return lx, ly end
end

local function physical_to_logical(px, py, osd_w, osd_h, rot)
    local x = px / osd_w
    local y = py / osd_h

    if rot == 90 then return y, 1 - x
    elseif rot == 180 then return 1 - x, 1 - y
    elseif rot == 270 then return 1 - y, x
    else return x, y end
end

local function draw_overlay()
    local osd_w, osd_h = mp.get_osd_size()
    if osd_w <= 0 or osd_h <= 0 then return end

    overlay.res_x = osd_w
    overlay.res_y = osd_h

    local paused = mp.get_property_bool("pause", false)
    local rot = get_rotation()
    local ass_rot = -rot

    -- Map logical zones to physical pixels
    local p_left_x, p_left_y = logical_to_physical(0.25, 0.5, rot)
    local p_right_x, p_right_y = logical_to_physical(0.75, 0.5, rot)

    p_left_x = p_left_x * osd_w
    p_left_y = p_left_y * osd_h
    p_right_x = p_right_x * osd_w
    p_right_y = p_right_y * osd_h

    -- Using standard ASCII text to bypass missing font packages on Pi OS Lite
    local play_text = paused and "[ PLAY ]" or "[ PAUSE ]"

    local ass = ""

    -- Left visual side
    ass = ass .. string.format(
        "{\\an5\\pos(%d,%d)\\frz%d\\fs36\\b1\\bord3\\shad1\\c&HFFFFFF&}%s\n",
        p_left_x, p_left_y, ass_rot, play_text
    )
    
    -- Right visual side
    ass = ass .. string.format(
        "{\\an5\\pos(%d,%d)\\frz%d\\fs36\\b1\\bord3\\shad1\\c&H5555FF&}[ EXIT ]\n",
        p_right_x, p_right_y, ass_rot
    )

    overlay.data = ass
    overlay:update()
end

local function handle_tap(source)
    local nx, ny = mp.get_mouse_pos()
    local osd_w, osd_h = mp.get_osd_size()

    if not nx or not ny or osd_w <= 0 then return end

    local rot = get_rotation()
    local lx, ly = physical_to_logical(nx, ny, osd_w, osd_h, rot)

    if lx < 0.5 then
        mp.command("cycle pause")
    else
        mp.command("quit")
    end
    
    draw_overlay()
end

-- Bind touch/mouse events
mp.add_forced_key_binding("MBTN_LEFT", "touch-tap", function() handle_tap("MBTN_LEFT") end, { repeatable = false })
mp.add_forced_key_binding("MOUSE_BTN0", "mouse-tap", function() handle_tap("MOUSE_BTN0") end, { repeatable = false })

mp.observe_property("pause", "bool", function() draw_overlay() end)
mp.register_event("file-loaded", function() draw_overlay() end)