local mp = require 'mp'
local msg = require 'mp.msg'

local overlay = mp.create_osd_overlay("ass-events")

local function format_time(seconds)
    if not seconds or seconds < 0 then return "00:00" end
    local m = math.floor(seconds / 60)
    local s = math.floor(seconds % 60)
    return string.format("%02d:%02d", m, s)
end

local last_draw_pos = -1
local last_draw_paused = nil

local function add_icon(id, vx, vy, filename, w, h)
    local pw, ph = mp.get_osd_size()
    if pw <= 0 or ph <= 0 then return end
    
    -- Calculate the visual Top-Right corner because the image was rotated 90 CCW.
    -- The original Top-Left of the image is now at the Top-Right of the visual space.
    local vtr_x = vx + w/2
    local vtr_y = vy - h/2
    
    local rx = vtr_x / 640.0
    local ry = vtr_y / 480.0
    
    local nx = math.floor(ry * pw)
    local ny = math.floor(ph - rx * ph)
    
    local file_path = "/home/pi/lumapi-cam/ui/assets/img_bgra/" .. filename
    
    mp.command_native({
        "overlay-add",
        id,
        nx, ny,
        file_path,
        0, "bgra",
        w, h, w*4
    })
end

local function draw_overlay(force)
    local pw, ph = mp.get_osd_size()
    if pw <= 0 or ph <= 0 then return end

    overlay.res_x = pw
    overlay.res_y = ph

    local time_pos = mp.get_property_number("time-pos") or 0
    local duration = mp.get_property_number("duration") or 0
    local paused = mp.get_property_bool("pause", false)

    if not force and last_draw_paused == paused and math.abs(time_pos - last_draw_pos) < 0.25 then
        return
    end

    last_draw_pos = time_pos
    last_draw_paused = paused

    -- Icons (resized to 64x64)
    add_icon(1, 600, 40, "exit.bgra", 64, 64)
    add_icon(2, 220, 430, "rewind.bgra", 64, 64)
    add_icon(3, 420, 430, "forward.bgra", 64, 64)
    if paused then
        add_icon(4, 320, 430, "play.bgra", 64, 64)
    else
        add_icon(4, 320, 430, "pause.bgra", 64, 64)
    end

    local text_rot = 90
    local function ass_text(vx, vy, content)
        local rx = vx / 640.0
        local ry = vy / 480.0
        local nx = ry * pw
        local ny = ph - (rx * ph)
        return string.format("{\\an5\\pos(%d,%d)\\frz%d}%s\n", nx, ny, text_rot, content)
    end

    local time_str = format_time(time_pos) .. " / " .. format_time(duration)
    local ass = ass_text(320, 350, "{\\c&HFFFFFF&\\1a&H00&\\fs28\\b1}" .. time_str)

    overlay.data = ass
    overlay:update()
end

mp.register_event("file-loaded", function() draw_overlay(true) end)
mp.observe_property("pause", "bool", function() draw_overlay(true) end)
mp.observe_property("time-pos", "number", function() draw_overlay(false) end)
mp.observe_property("duration", "number", function() draw_overlay(true) end)