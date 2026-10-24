-- TV-style player controls for CatTube's player (mpv): OK brings the
-- controls, OK again pauses, like a normal TV player. Loaded by CatTube
-- with --script.
--
--   controls hidden:  OK / Up / Down  show the controls (Pause focused)
--                     Left / Right    scrub 10 s back / forward
--                     Back            stop
--   controls shown:   Left / Right    move over the buttons: Previous /
--                                     Next video on the left (when there
--                                     is one), −10 s / Pause / +10 s in the
--                                     middle, Subtitles / Audio / Stop right
--                     OK              press the button (Pause first)
--                     Up              focus the seek bar: Left / Right scrub
--                                     (10 s, 60 s when held), Down returns
--                     Back            hide the controls
--   The bar hides itself 4 s after the last key while playing; it stays
--   while paused.
--   Subtitles / Audio open a list drawn here: Up/Down move, OK
--   picks ("Off" first for subtitles), Back closes. External subtitle
--   files get their server names from --script-opts=cattube_osd-subs=a;b.
--   Up next: CatTube's player loop decides everything (when
--   the credits start, the countdown, "Are you still watching?") and tells
--   this script what to draw with `script-message-to cattube_osd
--   cattube-card next LABEL LEFT [THUMB W H] | count LEFT | still LABEL
--   [THUMB W H] | hide`. While the card shows, Left / Right move between its two
--   buttons, OK presses one, Back cancels; the choice goes back as
--   `script-message cattube-card-result play|cancel|continue|stop`. Every
--   key also sends `cattube-input` (the "still watching" counter). New
--   subtitle names for the next episode arrive as `cattube-subs A;B`.
--
-- Drawn as ASS on a 1920x1080 canvas (scaled by mpv), accent from
-- --script-opts=cattube_osd-accent=RRGGBB.

local mp = require "mp"
local options = require "mp.options"
local assdraw = require "mp.assdraw"

local opts = { accent = "7c6cf0", subs = "" }
options.read_options(opts, "cattube_osd")

-- Names for external subtitle tracks, in the order they were added.
local sub_names = {}
for name in string.gmatch(opts.subs, "[^;]+") do
    sub_names[#sub_names + 1] = name
end

local menu = nil -- { kind = "sub"|"audio", items = {...}, index = n }
local card = nil -- { kind = "next"|"still", label, left, focus, thumb }

local W, H = 1920, 1080
local shown = false
local focus = 1 -- index into visible_buttons(); set to Pause when shown
local on_bar = false -- the seek bar has focus
local scrubs = 0 -- consecutive scrubs while held: step grows
local hide_timer = nil
local tick_timer = nil

local function seek(secs)
    mp.commandv("seek", secs, "relative")
end

local open_menu -- defined below (the buttons table refers to it)
local open_comments -- likewise
local toggle_chat -- likewise
local chat_on = false -- a live video's chat shows on the right

local function episode(which)
    mp.commandv("script-message", "cattube-episode", which)
end

-- Three groups: episodes on the left, playback in the middle, tracks on
-- the right. The episode buttons show only when there is one
-- (`cattube-episodes PREV NEXT`, each "yes" or "no").
-- Its label carries the speed when it is not normal ("Speed 1.5×").
local speed_button = { label = "Speed", group = "track", act = function() open_menu("speed") end }
-- Shown for a live video with a chat (`cattube-chat-available yes`).
local chat_button = { label = "Chat", group = "track", hidden = true, act = function() toggle_chat() end }
local pause_button = { label = "Pause", group = "play", act = function() mp.commandv("cycle", "pause") end }
local buttons = {
    { label = "Previous video",   group = "ep", hidden = true, act = function() episode("prev") end },
    { label = "Next video",       group = "ep", hidden = true, act = function() episode("next") end },
    { label = "−10 s",            group = "play", act = function() seek(-10) end },
    pause_button,
    { label = "+10 s",            group = "play", act = function() seek(10) end },
    speed_button,
    { label = "Quality",          group = "track", act = function() open_menu("quality") end },
    { label = "Comments",         group = "track", act = function() open_comments() end },
    chat_button,
    { label = "Subtitles",        group = "track", act = function() open_menu("sub") end },
    { label = "Audio",            group = "track", act = function() open_menu("audio") end },
    { label = "Stop",             group = "track", act = function() mp.commandv("quit") end },
}

local function visible_buttons()
    local v = {}
    for _, b in ipairs(buttons) do
        if not b.hidden then v[#v + 1] = b end
    end
    return v
end

local function pause_index()
    for i, b in ipairs(visible_buttons()) do
        if b == pause_button then return i end
    end
    return 1
end

-- "RRGGBB" → ASS "&HBBGGRR&"
local function ass_rgb(hex)
    hex = hex:gsub("#", "")
    if #hex ~= 6 then hex = "7c6cf0" end
    return string.format("&H%s%s%s&", hex:sub(5, 6), hex:sub(3, 4), hex:sub(1, 2))
end
local ACCENT = ass_rgb(opts.accent)

local function clock(secs)
    if not secs or secs < 0 then secs = 0 end
    secs = math.floor(secs + 0.5)
    local h = math.floor(secs / 3600)
    local m = math.floor(secs % 3600 / 60)
    local s = secs % 60
    if h > 0 then
        return string.format("%d:%02d:%02d", h, m, s)
    end
    return string.format("%d:%02d", m, s)
end

local function rect(a, x, y, w, h, r)
    -- Rounded rectangle (r = corner radius) as an ASS drawing.
    r = math.min(r, w / 2, h / 2)
    a:move_to(x + r, y)
    a:line_to(x + w - r, y)
    a:bezier_curve(x + w, y, x + w, y, x + w, y + r)
    a:line_to(x + w, y + h - r)
    a:bezier_curve(x + w, y + h, x + w, y + h, x + w - r, y + h)
    a:line_to(x + r, y + h)
    a:bezier_curve(x, y + h, x, y + h, x, y + h - r)
    a:line_to(x, y + r)
    a:bezier_curve(x, y, x, y, x + r, y)
end

local function text_width(label, size)
    -- Rough: the font is proportional, 0.52 em per character is close for
    -- the labels used here.
    return #label * size * 0.52
end

local render_menu, render_card, render_chat -- below

-- Everything drawn goes through here, so the Up next card is drawn
-- over whatever else shows.
local function set_osd(text)
    -- The chat goes under everything else, and steps aside for a menu.
    if chat_on and render_chat and not menu then
        local c = assdraw.ass_new()
        render_chat(c)
        text = c.text .. (text ~= "" and ("\n" .. text) or "")
    end
    if card then
        local c = assdraw.ass_new()
        render_card(c)
        text = (text ~= "" and (text .. "\n") or "") .. c.text
    end
    mp.set_osd_ass(W, H, text)
end

-- SponsorBlock segments of the playing video ({from, to} seconds), sent
-- by CatTube for each video ("cattube-segments 31.2-61.5,90-95").
local segments = {}
mp.register_script_message("cattube-segments", function(list)
    segments = {}
    for a, b in (list or ""):gmatch("([%d.]+)-([%d.]+)") do
        segments[#segments + 1] = { tonumber(a), tonumber(b) }
    end
end)

local function chapter_title()
    local i = mp.get_property_number("chapter", -1)
    local list = mp.get_property_native("chapter-list", {})
    local c = list[i + 1]
    if c and c.title and c.title ~= "" then
        return c.title:gsub("[{}]", "")
    end
    return nil
end

-- Subtitles move up above the controls while they show.
local sub_margin = nil
local function lift_subs()
    if sub_margin == nil then sub_margin = mp.get_property_number("sub-margin-y", 22) end
    local want = shown and 190 or sub_margin
    if mp.get_property_number("sub-margin-y", want) ~= want then
        mp.set_property_number("sub-margin-y", want)
    end
end

local function render()
    lift_subs()
    if not shown and not menu then
        set_osd("")
        return
    end
    if not shown then
        local a = assdraw.ass_new()
        render_menu(a)
        set_osd(a.text)
        return
    end
    local pos = mp.get_property_number("time-pos", 0)
    local dur = mp.get_property_number("duration", 0)
    local paused = mp.get_property_native("pause", false)
    local title = mp.get_property("media-title", "")
    pause_button.label = paused and "Play" or "Pause"

    local a = assdraw.ass_new()
    -- The picture darkens towards the bottom so the controls read on
    -- anything: a wide soft fade, then a deeper band behind the buttons.
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\blur140\\1c&H000000&\\1a&H60&}")
    a:draw_start()
    a:rect_cw(-200, 700, W + 200, H + 200)
    a:draw_stop()
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\blur70\\1c&H000000&\\1a&H50&}")
    a:draw_start()
    a:rect_cw(-200, 880, W + 200, H + 200)
    a:draw_stop()
    -- Title: "Series · S1:E3 · Name" becomes a small "Series · S1:E3"
    -- over the episode's name.
    local clean = title:gsub("[{}]", "")
    local series, ep, name = clean:match("^(.-) · (S%d+:E%d+) · (.+)$")
    if series then
        a:new_event()
        a:append("{\\an1\\pos(120,792)\\bord0\\shad0\\fs28\\1c&HFFFFFF&\\1a&H50&}" .. series .. " · " .. ep)
        clean = name
    end
    a:new_event()
    a:append("{\\an1\\pos(120,846)\\bord0\\shad0\\fs44\\b1\\1c&HFFFFFF&\\1a&H08&}" .. clean)
    -- When it ends, like a TV guide (a stream: that it is live).
    if mp.get_property("metadata/by-key/ytdl_is_live", "") == "true" then
        a:new_event()
        a:append("{\\an3\\pos(1800,846)\\bord0\\shad0\\fs28\\b1\\1c&H4444FF&\\1a&H00&}● LIVE")
    elseif dur > 0 then
        local speed = mp.get_property_number("speed", 1)
        local ends = os.time() + math.max(0, dur - pos) / (speed > 0 and speed or 1)
        a:new_event()
        a:append("{\\an3\\pos(1800,846)\\bord0\\shad0\\fs28\\1c&HFFFFFF&\\1a&H40&}Ends at " .. os.date("%H:%M", math.floor(ends)))
    end
    -- Progress track and fill.
    local tx, ty, tw, th = 120, 874, 1680, 8
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&HFFFFFF&\\1a&HB0&}")
    a:draw_start()
    rect(a, tx, ty, tw, th, 4)
    a:draw_stop()
    local frac = dur > 0 and math.max(0, math.min(1, pos / dur)) or 0
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c" .. ACCENT .. "\\1a&H00&}")
    a:draw_start()
    rect(a, tx, ty, math.max(8, tw * frac), th, 4)
    a:draw_stop()
    -- SponsorBlock segments in its green (over the played part too).
    if dur > 0 and #segments > 0 then
        a:new_event()
        a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H00D400&\\1a&H30&}")
        a:draw_start()
        for _, sg in ipairs(segments) do
            local x1 = tx + tw * math.max(0, math.min(1, sg[1] / dur))
            local x2 = tx + tw * math.max(0, math.min(1, sg[2] / dur))
            if x2 - x1 >= 1 then
                a:rect_cw(x1, ty, x2, ty + th)
            end
        end
        a:draw_stop()
    end
    -- Chapters: a small gap in the track where each one starts.
    local chapters = dur > 0 and mp.get_property_native("chapter-list", {}) or {}
    if #chapters > 1 then
        a:new_event()
        a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H000000&\\1a&H00&}")
        a:draw_start()
        for i = 2, #chapters do
            local x = tx + tw * math.max(0, math.min(1, (chapters[i].time or 0) / dur))
            a:rect_cw(x - 2, ty - 1, x + 2, ty + th + 1)
        end
        a:draw_stop()
    end
    -- Knob (bigger, with an accent ring, while the bar has focus).
    local kr = on_bar and 14 or 10
    if on_bar then
        a:new_event()
        a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c" .. ACCENT .. "\\1a&H60&}")
        a:draw_start()
        a:round_rect_cw(tx + tw * frac - 22, ty - 18, tx + tw * frac + 22, ty + th + 18, 22)
        a:draw_stop()
    end
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&HFFFFFF&\\1a&H00&}")
    a:draw_start()
    a:round_rect_cw(tx + tw * frac - kr, ty + th / 2 - kr, tx + tw * frac + kr, ty + th / 2 + kr, kr)
    a:draw_stop()
    -- Times.
    a:new_event()
    local chapter = chapter_title()
    a:append(string.format("{\\an7\\pos(120,898)\\bord0\\shad0\\fs30\\1c&HFFFFFF&\\1a&H30&}%s", clock(pos) .. (chapter and ("  ·  " .. chapter) or "")))
    a:new_event()
    a:append(string.format("{\\an9\\pos(1800,898)\\bord0\\shad0\\fs30\\1c&HFFFFFF&\\1a&H30&}%s", clock(dur)))
    -- Buttons: episodes from the left edge, playback centred, tracks to
    -- the right edge.
    -- Smaller when they would not fit side by side (both video buttons,
    -- a long speed label).
    local size, pad, gap, bh = 28, 40, 16, 60
    local shown_buttons = visible_buttons()
    local widths, group_w
    for _, v in ipairs({ { 28, 40, 16 }, { 25, 30, 12 }, { 22, 24, 10 } }) do
        size, pad, gap = v[1], v[2], v[3]
        widths, group_w = {}, { ep = 0, play = 0, track = 0 }
        for i, b in ipairs(shown_buttons) do
            widths[i] = text_width(b.label, size) + pad
            group_w[b.group] = group_w[b.group] + widths[i] + (group_w[b.group] > 0 and gap or 0)
        end
        if group_w.ep + group_w.play + group_w.track + 2 * 48 <= W - 240 then break end
    end
    local start = { ep = 120, play = (W - group_w.play) / 2, track = W - 120 - group_w.track }
    -- Not enough room to centre playback: it lines up on the left (after
    -- the previous / next buttons), the rest stays on the right.
    local space = 48
    if start.play + group_w.play + space > start.track then
        start.play = 120 + (group_w.ep > 0 and group_w.ep + space or 0)
    end
    -- Well clear of the bottom edge (TVs crop it).
    local by = 952
    for i, b in ipairs(shown_buttons) do
        local x = start[b.group]
        start[b.group] = x + widths[i] + gap
        local focused = i == focus and not on_bar
        a:new_event()
        if focused then
            a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c" .. ACCENT .. "\\1a&H00&}")
        else
            -- Solid dark, with a hairline: never tinted by the picture.
            a:append("{\\an7\\pos(0,0)\\bord1.5\\shad0\\3c&HFFFFFF&\\3a&HC8&\\1c&H14100E&\\1a&H48&}")
        end
        a:draw_start()
        rect(a, x, by, widths[i], bh, bh / 2)
        a:draw_stop()
        a:new_event()
        a:append(string.format("{\\an5\\pos(%d,%d)\\bord0\\shad0\\fs%d\\b%d\\1c&H%s&\\1a&H00&}%s",
            math.floor(x + widths[i] / 2), math.floor(by + bh / 2), size, focused and 1 or 0,
            focused and "101018" or "FFFFFF", b.label))
    end
    if menu then render_menu(a) end
    set_osd(a.text)
end

-- ---------------------------------------------------------------- menus

local LANGS = {
    eng = "English", en = "English", deu = "German", ger = "German", de = "German",
    fra = "French", fre = "French", fr = "French", spa = "Spanish", es = "Spanish",
    ita = "Italian", it = "Italian", por = "Portuguese", pt = "Portuguese",
    rus = "Russian", ru = "Russian", jpn = "Japanese", ja = "Japanese",
    kor = "Korean", ko = "Korean", zho = "Chinese", chi = "Chinese", zh = "Chinese",
    hin = "Hindi", hi = "Hindi", ara = "Arabic", ar = "Arabic", nld = "Dutch",
    dut = "Dutch", nl = "Dutch", pol = "Polish", pl = "Polish", tur = "Turkish",
    tr = "Turkish", ukr = "Ukrainian", uk = "Ukrainian", ind = "Indonesian",
    id = "Indonesian", vie = "Vietnamese", vi = "Vietnamese",
}

local function track_label(t, external_n)
    local label = t.title
    if (not label or label == "") and t.external and sub_names[external_n] then
        label = sub_names[external_n]
    end
    if not label or label == "" then
        label = t.lang and t.lang ~= "" and (LANGS[t.lang:lower()] or t.lang) or ("Track " .. tostring(t.id))
    end
    if t.forced then label = label .. " · forced" end
    return label
end

-- Speeds and the highest qualities offered; a quality change reloads the
-- video where it is (CatTube does that).
local SPEEDS = { 0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 3 }
local QUALITIES = { { "Best", 0 }, { "4K", 2160 }, { "1440p", 1440 }, { "1080p", 1080 }, { "720p", 720 }, { "480p", 480 } }

-- What YouTube would call the picture playing: by its width for wide
-- pictures (2048x858 is 1080p), by the width for upright ones too.
local function quality_class()
    local w = mp.get_property_number("width", 0)
    local h = mp.get_property_number("height", 0)
    if w <= 0 or h <= 0 then return 0 end
    local q = w >= h and w * 9 / 16 or w
    local class = 144
    for _, c in ipairs({ 144, 240, 360, 480, 720, 1080, 1440, 2160, 4320 }) do
        if q >= c * 0.94 then class = c end
    end
    return class
end

local function build_menu(kind)
    local items = {}
    local title = nil
    if kind == "speed" then
        local cur = mp.get_property_number("speed", 1)
        for _, sp in ipairs(SPEEDS) do
            items[#items + 1] = { label = (sp == 1 and "Normal" or (tostring(sp) .. "×")), id = tostring(sp), selected = math.abs(sp - cur) < 0.001 }
        end
    elseif kind == "quality" then
        -- The cap in use, from the format CatTube asked yt-dlp for.
        local cap = tonumber((mp.get_property("ytdl-format", "")):match("height<=(%d+)")) or 0
        for _, q in ipairs(QUALITIES) do
            items[#items + 1] = { label = q[1], id = tostring(q[2]), selected = q[2] == cap }
        end
        local now = quality_class()
        title = "Quality" .. (now > 0 and (" · now " .. tostring(now) .. "p") or "")
    end
    if kind == "speed" or kind == "quality" then
        local index = 1
        for i, it in ipairs(items) do
            if it.selected then index = i end
        end
        return { kind = kind, items = items, index = index, title = title }
    end
    local tracks = mp.get_property_native("track-list", {})
    local current = mp.get_property(kind == "sub" and "sid" or "aid", "no")
    if kind == "sub" then
        items[1] = { label = "Off", id = "no", selected = current == "no" }
    end
    local external_n = 0
    for _, t in ipairs(tracks) do
        if (kind == "sub" and t.type == "sub") or (kind == "audio" and t.type == "audio") then
            if t.external then external_n = external_n + 1 end
            items[#items + 1] = {
                label = track_label(t, external_n),
                id = tostring(t.id),
                selected = t.selected == true,
            }
        end
    end
    local index = 1
    for i, it in ipairs(items) do
        if it.selected then index = i end
    end
    return { kind = kind, items = items, index = index }
end

open_menu = function(kind)
    menu = build_menu(kind)
    if #menu.items == 0 then
        mp.osd_message(kind == "sub" and "No subtitles in this video" or "No audio tracks", 1.5)
        menu = nil
        return
    end
    render()
end

local function menu_pick()
    if menu.kind == "comments" then return end
    local it = menu.items[menu.index]
    if not it then menu = nil; render(); return end
    if menu.kind == "speed" then
        mp.set_property("speed", it.id)
    elseif menu.kind == "quality" then
        mp.commandv("script-message", "cattube-quality", it.id)
    elseif menu.kind == "sub" then
        mp.set_property("sid", it.id)
        if it.id ~= "no" then mp.set_property("sub-visibility", "yes") end
        mp.osd_message(it.id == "no" and "Subtitles off" or ("Subtitles: " .. it.label), 1.5)
    else
        mp.set_property("aid", it.id)
        mp.osd_message("Audio: " .. it.label, 1.5)
    end
    menu = nil
    render()
end

-- ------------------------------------------------------------ comments

-- A video's comments, fetched by CatTube page by page ("cattube-comments
-- first|<token>" → "cattube-comments-data <json>").
local utils = require "mp.utils"
local comments = { path = nil, items = {}, more = "", loading = false, error = nil }

local function comments_request(arg)
    comments.loading = true
    mp.commandv("script-message", "cattube-comments", arg)
end

mp.register_script_message("cattube-comments-data", function(json)
    comments.loading = false
    local d = utils.parse_json(json or "")
    if not d then
        comments.error = "YouTube didn't answer properly"
    elseif d.error then
        comments.error = d.error
    else
        for _, c in ipairs(d.list or {}) do
            comments.items[#comments.items + 1] = c
        end
        comments.more = d.more or ""
        comments.error = nil
    end
    if menu and menu.kind == "comments" then render() end
end)

open_comments = function()
    local path = mp.get_property("path", "")
    if comments.path ~= path then
        comments = { path = path, items = {}, more = "", loading = false, error = nil }
        comments_request("first")
    end
    menu = { kind = "comments", items = comments.items, index = 1 }
    render()
end

-- Near the end of the list: the next page.
local function comments_moved()
    if menu.index >= #menu.items - 2 and comments.more ~= "" and not comments.loading then
        comments_request(comments.more)
    end
end

-- Characters on screen (UTF-8 continuation bytes do not count).
local function chars(t)
    return select(2, t:gsub("[^\128-\191]", ""))
end

-- Word-wrapped lines of `t`, at most `max` (the last cut with "…").
local function wrap(t, size, width, max)
    local per = math.floor(width / (size * 0.5))
    local out = {}
    for para in (t .. "\n"):gmatch("(.-)\n") do
        local line = ""
        for word in para:gmatch("%S+") do
            if line == "" then
                line = word
            elseif chars(line) + 1 + chars(word) <= per then
                line = line .. " " .. word
            else
                out[#out + 1] = line
                line = word
            end
        end
        if line ~= "" then out[#out + 1] = line end
    end
    if #out > max then
        local cut = {}
        for i = 1, max do cut[i] = out[i] end
        cut[max] = cut[max] .. " …"
        return cut
    end
    return out
end

local function render_comments(a)
    local px, py, pw, ph = 1160, 60, 700, 740
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H101018&\\1a&H20&}")
    a:draw_start()
    rect(a, px, py, pw, ph, 22)
    a:draw_stop()
    a:new_event()
    a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs34\\b1\\1c&HFFFFFF&\\1a&H00&}Comments", px + 32, py + 28))
    local n = #menu.items
    local function note(text)
        a:new_event()
        a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs26\\1c&HFFFFFF&\\1a&H50&}%s", px + 32, py + 100, text))
    end
    if n == 0 then
        note(comments.error and comments.error:gsub("[{}\\]", "") or "Loading comments…")
        return
    end
    a:new_event()
    a:append(string.format("{\\an9\\pos(%d,%d)\\bord0\\shad0\\fs22\\1c&HFFFFFF&\\1a&H60&}%d / %d%s",
        px + pw - 28, py + 40, menu.index, n, comments.more ~= "" and "+" or ""))
    local y, bottom = py + 96, py + ph - 24
    local tw = pw - 88
    for i = menu.index, n do
        local c = menu.items[i]
        local text = (c.t or ""):gsub("[{}\\]", "")
        local lines = wrap(text, 24, tw, i == menu.index and 9 or 4)
        local h = 36 + #lines * 32 + 22
        if y + h > bottom and i > menu.index then break end
        if i == menu.index then
            a:new_event()
            a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c" .. ACCENT .. "\\1a&H00&}")
            a:draw_start()
            rect(a, px + 24, y + 2, 6, h - 24, 3)
            a:draw_stop()
        end
        local meta = (c.p or "")
        if c.l and c.l ~= "" then meta = meta .. "  ·  " .. c.l .. " likes" end
        if c.r and c.r > 0 then meta = meta .. "  ·  " .. c.r .. (c.r == 1 and " reply" or " replies") end
        a:new_event()
        a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs24\\b1\\1c&HFFFFFF&\\1a&H00&}%s{\\b0\\1a&H70&}   %s",
            px + 48, y, (c.a or ""):gsub("[{}\\]", ""), meta:gsub("[{}\\]", "")))
        for li, line in ipairs(lines) do
            a:new_event()
            a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs24\\1c&HFFFFFF&\\1a&H%s&}%s",
                px + 48, y + 4 + li * 32, i == menu.index and "00" or "40", line))
        end
        y = y + h
        if y > bottom then break end
    end
    if comments.loading then
        a:new_event()
        a:append(string.format("{\\an1\\pos(%d,%d)\\bord0\\shad0\\fs22\\1c&HFFFFFF&\\1a&H60&}Loading more…", px + 48, py + ph - 12))
    end
end

-- ------------------------------------------------------------ live chat

-- Messages arrive from CatTube while the chat is on
-- ("cattube-chat-data [{a, t, p}]"), newest last.
local chat_messages = {}

mp.register_script_message("cattube-chat-available", function(yes)
    chat_button.hidden = yes ~= "yes"
    if yes ~= "yes" then
        chat_on = false
        chat_button.label = "Chat"
        chat_messages = {}
    end
    render()
end)

mp.register_script_message("cattube-chat-data", function(json)
    for _, m in ipairs(utils.parse_json(json or "") or {}) do
        chat_messages[#chat_messages + 1] = m
    end
    while #chat_messages > 60 do table.remove(chat_messages, 1) end
    if chat_on then render() end
end)

toggle_chat = function()
    chat_on = not chat_on
    chat_button.label = chat_on and "Hide chat" or "Chat"
    mp.commandv("script-message", "cattube-chat", chat_on and "on" or "off")
    render()
end

render_chat = function(a)
    local px, pw, top = 1400, 460, 80
    local bottom = shown and 740 or 1010
    -- Newest at the bottom: lay out from the end upwards.
    local blocks, y = {}, bottom - 20
    for i = #chat_messages, 1, -1 do
        local m = chat_messages[i]
        local author = (m.a or ""):gsub("[{}\\]", "")
        local paid = (m.p or "") ~= "" and ("  " .. m.p:gsub("[{}\\]", "")) or ""
        local lines = wrap(author .. paid .. "  " .. (m.t or ""):gsub("[{}\\]", ""), 22, pw - 48, 3)
        local h = #lines * 30 + 10
        if y - h < top + 20 then break end
        y = y - h
        blocks[#blocks + 1] = { y = y, lines = lines, author = author .. paid }
    end
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H101018&\\1a&H50&}")
    a:draw_start()
    rect(a, px, top, pw, bottom - top, 20)
    a:draw_stop()
    if #blocks == 0 then
        a:new_event()
        a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs22\\1c&HFFFFFF&\\1a&H60&}Waiting for the chat…", px + 24, top + 24))
        return
    end
    for _, b in ipairs(blocks) do
        for li, line in ipairs(b.lines) do
            a:new_event()
            if li == 1 and line:sub(1, #b.author) == b.author then
                -- The author in the accent colour, the message in white.
                a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs22\\b1\\1c%s\\1a&H00&}%s{\\b0\\1c&HFFFFFF&}%s",
                    px + 24, b.y + (li - 1) * 30, ACCENT, b.author, line:sub(#b.author + 1)))
            else
                a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs22\\1c&HFFFFFF&\\1a&H00&}%s",
                    px + 24, b.y + (li - 1) * 30, line))
            end
        end
    end
end

-- The list panel on the right (drawn over the bar).
render_menu = function(a)
    if menu.kind == "comments" then return render_comments(a) end
    local px, py, pw = 1300, 140, 540
    local rh, visible = 66, 9
    local n = #menu.items
    local first = math.max(1, math.min(menu.index - visible + 1, n - visible + 1))
    if menu.index < first then first = menu.index end
    local shown_n = math.min(visible, n - first + 1)
    local ph = 96 + shown_n * rh + 24
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H101018&\\1a&H28&}")
    a:draw_start()
    rect(a, px, py, pw, ph, 22)
    a:draw_stop()
    a:new_event()
    a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs34\\b1\\1c&HFFFFFF&\\1a&H00&}%s",
        px + 32, py + 28, menu.title or ({ sub = "Subtitles", audio = "Audio", speed = "Speed", quality = "Quality" })[menu.kind] or ""))
    for row = 0, shown_n - 1 do
        local i = first + row
        local it = menu.items[i]
        local y = py + 96 + row * rh
        local focused = i == menu.index
        if focused then
            a:new_event()
            a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c" .. ACCENT .. "\\1a&H00&}")
            a:draw_start()
            rect(a, px + 16, y, pw - 32, rh - 8, 14)
            a:draw_stop()
        end
        if it.selected then
            a:new_event()
            a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H" .. (focused and "101018" or "FFFFFF") .. "&\\1a&H00&}")
            a:draw_start()
            a:round_rect_cw(px + 38, y + (rh - 8) / 2 - 7, px + 52, y + (rh - 8) / 2 + 7, 7)
            a:draw_stop()
        end
        local label = it.label:gsub("[{}]", "")
        if #label > 34 then label = label:sub(1, 33) .. "…" end
        a:new_event()
        a:append(string.format("{\\an4\\pos(%d,%d)\\bord0\\shad0\\fs28\\b%d\\1c&H%s&\\1a&H00&}%s",
            px + 70, math.floor(y + (rh - 8) / 2), focused and 1 or 0,
            focused and "101018" or "FFFFFF", label))
    end
    if n > visible then
        a:new_event()
        a:append(string.format("{\\an9\\pos(%d,%d)\\bord0\\shad0\\fs22\\1c&HFFFFFF&\\1a&H60&}%d / %d",
            px + pw - 28, py + 40, menu.index, n))
    end
end

-- ------------------------------------------------------- next episode

-- The card sits bottom right (above the control bar when it shows).
local CW, CH = 720, 200
local TW, TH = 256, 144

local function card_box()
    local x = W - 56 - CW
    local y = H - 56 - CH
    if shown then y = y - 250 end
    return x, y
end

local function clean(t)
    t = (t or ""):gsub("[{}\\]", "")
    return t
end

-- Characters, not bytes ("·" is two bytes); cut at a character boundary.
local function fit(t, size, width)
    local max = math.floor(width / (size * 0.45))
    local n = utf8 and utf8.len(t) or #t
    if n and n > max then
        local cut = utf8 and utf8.offset(t, max) or max
        t = t:sub(1, cut - 1) .. "…"
    end
    return t
end

local function pill(a, x, y, w, h, label, focused)
    a:new_event()
    if focused then
        a:append("{\\an7\\pos(0,0)\\bord1.5\\shad0\\1c" .. ACCENT .. "\\1a&H00&\\3c&HFFFFFF&\\3a&H70&}")
    else
        a:append("{\\an7\\pos(0,0)\\bord1\\shad0\\1c&HFFFFFF&\\1a&HE6&\\3c&HFFFFFF&\\3a&HC8&}")
    end
    a:draw_start()
    rect(a, x, y, w, h, h / 2)
    a:draw_stop()
    a:new_event()
    a:append(string.format("{\\an5\\pos(%d,%d)\\bord0\\shad0\\fs26\\b%d\\1c&HFFFFFF&\\1a&H%s&}%s",
        x + w / 2, y + h / 2, focused and 1 or 0, focused and "00" or "30", label))
end

local thumb_at = nil -- where the thumbnail overlay was put (x, y)

local function place_thumb(x, y)
    if not card or not card.thumb then
        if thumb_at then mp.commandv("overlay-remove", 7); thumb_at = nil end
        return
    end
    local dims = mp.get_property_native("osd-dimensions") or {}
    local sx = (dims.w or W) / W
    local sy = (dims.h or H) / H
    local px, py = math.floor(x * sx), math.floor(y * sy)
    if thumb_at and thumb_at[1] == px and thumb_at[2] == py then return end
    local t = card.thumb
    mp.commandv("overlay-add", 7, px, py, t.path, 0, "bgra", t.w, t.h, t.w * 4)
    thumb_at = { px, py }
end

render_card = function(a)
    local x, y = card_box()
    -- Dark glass, a thin light edge, the frosted band along the top.
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord1.5\\shad0\\1c&H241814&\\1a&H38&\\3c&HFFFFFF&\\3a&HB4&}")
    a:draw_start()
    rect(a, x, y, CW, CH, 28)
    a:draw_stop()
    -- Frost: faint layers from the top edge that add up to a soft fade.
    for k = 1, 4 do
        a:new_event()
        a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&HFFFFFF&\\1a&HFA&}")
        a:draw_start()
        rect(a, x + 1, y + 1, CW - 2, 22 * k, 27)
        a:draw_stop()
    end
    -- Thumbnail frame (the picture is an overlay placed on top of it).
    local tx, ty = x + 28, y + 28
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&H000000&\\1a&H60&}")
    a:draw_start()
    rect(a, tx, ty, TW, TH, 14)
    a:draw_stop()
    place_thumb(tx, ty)
    local cx = tx + TW + 28
    local cw = CW - (cx - x) - 28
    local heading, label, b1, b2
    if card.kind == "still" then
        heading = "Are you still watching?"
        b1, b2 = "Continue", "Stop"
    else
        heading = "Up next in " .. tostring(card.left or 0)
        b1, b2 = "Play now", "Cancel"
    end
    label = fit(clean(card.label), 30, cw)
    a:new_event()
    a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs24\\1c&HFFFFFF&\\1a&H50&}%s", cx, y + 26, heading))
    a:new_event()
    a:append(string.format("{\\an7\\pos(%d,%d)\\bord0\\shad0\\fs30\\b1\\1c&HFFFFFF&\\1a&H00&}%s", cx, y + 60, label))
    pill(a, cx, y + 124, 196, 52, b1, card.focus == 1)
    pill(a, cx + 210, y + 124, 160, 52, b2, card.focus == 2)
end

local function card_result(r)
    mp.commandv("script-message", "cattube-card-result", r)
end

local function card_hide()
    card = nil
    place_thumb(0, 0)
end

mp.register_script_message("cattube-card", function(cmd, a1, a2, a3, a4, a5)
    if cmd == "next" then
        local thumb = nil
        if a3 and a3 ~= "" and tonumber(a4) and tonumber(a5) then
            thumb = { path = a3, w = tonumber(a4), h = tonumber(a5) }
        end
        card = { kind = "next", label = a1, left = tonumber(a2) or 0, focus = 1, thumb = thumb }
        thumb_at = nil
    elseif cmd == "count" and card then
        card.left = tonumber(a1) or card.left
    elseif cmd == "still" then
        local thumb = card and card.thumb or nil
        if a2 and a2 ~= "" and tonumber(a3) and tonumber(a4) then
            thumb = { path = a2, w = tonumber(a3), h = tonumber(a4) }
        end
        card = { kind = "still", label = a1, focus = 1, thumb = thumb }
        thumb_at = nil
    elseif cmd == "hide" then
        card_hide()
    end
    render()
end)

-- The player loop says which episode buttons apply:
-- `cattube-episodes PREV NEXT`, each "yes" or "no".
mp.register_script_message("cattube-episodes", function(has_prev, has_next)
    local focused = visible_buttons()[focus]
    buttons[1].hidden = has_prev ~= "yes"
    buttons[2].hidden = has_next ~= "yes"
    focus = pause_index()
    for i, b in ipairs(visible_buttons()) do
        if b == focused then focus = i end
    end
    if shown then render() end
end)

-- Subtitle names of the next episode's external tracks (same player).
mp.register_script_message("cattube-subs", function(names)
    sub_names = {}
    for name in string.gmatch(names or "", "[^;]+") do
        sub_names[#sub_names + 1] = name
    end
end)

local function card_ok()
    if card.kind == "still" then
        card_result(card.focus == 1 and "continue" or "stop")
    else
        card_result(card.focus == 1 and "play" or "cancel")
    end
end

local function card_move(dir)
    card.focus = (dir < 0) and 1 or 2
    render()
end

-- Every key counts as "someone is watching".
local function notify()
    mp.commandv("script-message", "cattube-input")
end

local function stop_timers()
    if hide_timer then hide_timer:kill(); hide_timer = nil end
    if tick_timer then tick_timer:kill(); tick_timer = nil end
end

local function hide()
    shown = false
    menu = nil
    on_bar = false
    scrubs = 0
    stop_timers()
    render()
end

local scrub_timer = nil
local function arm_hide()
    if hide_timer then hide_timer:kill() end
    hide_timer = mp.add_timeout(4, function()
        if menu then arm_hide() return end
        if not mp.get_property_native("pause", false) then hide() end
    end)
    -- A pause in scrubbing ends the held-key acceleration.
    if scrub_timer then scrub_timer:kill() end
    scrub_timer = mp.add_timeout(0.4, function() scrubs = 0 end)
end

local function show()
    -- Coming up from hidden (OK, an arrow, or a pause from a media key):
    -- focus starts on Play / Pause.
    if not shown then
        focus = pause_index()
        on_bar = false
    end
    shown = true
    render()
    if not tick_timer then
        tick_timer = mp.add_periodic_timer(0.5, render)
    end
    arm_hide()
end

local function on_ok()
    notify()
    if card then card_ok(); return end
    if menu then menu_pick(); return end
    if not shown then
        focus = pause_index()
        show()
        return
    end
    if on_bar then
        mp.commandv("cycle", "pause")
    else
        local b = visible_buttons()[focus]
        if b then b.act() end
    end
    render()
    arm_hide()
end

-- Scrubbing on the bar: 10 s a press, 60 s once it has been held a while.
local function scrub(dir)
    scrubs = scrubs + 1
    seek(dir * (scrubs > 6 and 60 or 10))
    render()
    arm_hide()
end

local function on_left()
    notify()
    if card then card_move(-1); return end
    if menu then return end
    if shown and on_bar then
        scrub(-1)
    elseif shown then
        focus = (focus - 2) % #visible_buttons() + 1
        render()
        arm_hide()
    else
        seek(-10)
        mp.commandv("show-progress")
    end
end

local function on_right()
    notify()
    if card then card_move(1); return end
    if menu then return end
    if shown and on_bar then
        scrub(1)
    elseif shown then
        focus = focus % #visible_buttons() + 1
        render()
        arm_hide()
    else
        seek(10)
        mp.commandv("show-progress")
    end
end

local function on_up()
    notify()
    if card then return end
    if menu and menu.kind == "comments" then
        menu.index = math.max(1, menu.index - 1)
        render()
        return
    end
    if menu then
        menu.index = (menu.index - 2) % #menu.items + 1
        render()
        return
    end
    if not shown then
        focus = pause_index()
        show()
    elseif not on_bar then
        on_bar = true
        scrubs = 0
        render()
        arm_hide()
    else
        hide()
    end
end

local function on_down()
    notify()
    if card then return end
    if menu and menu.kind == "comments" then
        menu.index = math.max(1, math.min(#menu.items, menu.index + 1))
        comments_moved()
        render()
        return
    end
    if menu then
        menu.index = menu.index % #menu.items + 1
        render()
        return
    end
    if not shown then
        focus = pause_index()
        show()
    elseif on_bar then
        on_bar = false
        render()
        arm_hide()
    else
        arm_hide()
    end
end

local function on_back()
    notify()
    if card then card_result("cancel"); return end
    if menu then menu = nil; render(); return end
    if shown then hide() else mp.commandv("quit") end
end

for _, k in ipairs({ "ENTER", "KP_ENTER", "SPACE" }) do
    mp.add_forced_key_binding(k, "cattube-ok-" .. k, on_ok)
end
mp.add_forced_key_binding("LEFT", "cattube-left", on_left, { repeatable = true })
mp.add_forced_key_binding("RIGHT", "cattube-right", on_right, { repeatable = true })
mp.add_forced_key_binding("UP", "cattube-up", on_up)
mp.add_forced_key_binding("DOWN", "cattube-down", on_down)
mp.add_forced_key_binding("ESC", "cattube-back", on_back)
mp.add_forced_key_binding("BS", "cattube-back-bs", on_back)
-- Media keys keep working through the input conf; the bar follows pause.
mp.observe_property("pause", "native", function(_, paused)
    if paused then show() elseif shown then arm_hide() end
    render()
end)

mp.observe_property("speed", "number", function(_, sp)
    if sp and math.abs(sp - 1) > 0.001 then
        speed_button.label = string.format("Speed %g×", sp)
    else
        speed_button.label = "Speed"
    end
end)

-- Loading: from the moment the player opens (and for each next video)
-- until its first picture, a spinning arc in the accent colour and the
-- video's title, instead of a black screen. Also while the video waits
-- for more data.
local loading_overlay = mp.create_osd_overlay("ass-events")
loading_overlay.res_x, loading_overlay.res_y = W, H
local loading_timer = nil
local loading_since = mp.get_time()

local function arc_ring(a, cx, cy, r_out, r_in, from, to)
    local steps = 40
    a:move_to(cx + r_out * math.cos(from), cy + r_out * math.sin(from))
    for i = 1, steps do
        local t = from + (to - from) * i / steps
        a:line_to(cx + r_out * math.cos(t), cy + r_out * math.sin(t))
    end
    for i = steps, 0, -1 do
        local t = from + (to - from) * i / steps
        a:line_to(cx + r_in * math.cos(t), cy + r_in * math.sin(t))
    end
end

local function draw_loading()
    local t = mp.get_time() - loading_since
    local a = assdraw.ass_new()
    -- Just the spinner, in the middle: no title while it loads.
    local cx, cy = W / 2, H / 2
    -- A faint track, then the arc: it turns, and grows and shrinks.
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c&HFFFFFF&\\1a&HE6&}")
    a:draw_start()
    arc_ring(a, cx, cy, 34, 28, 0, 2 * math.pi - 0.001)
    a:draw_stop()
    local spin = t * 5.2
    local len = 1.2 + 1.8 * (0.5 + 0.5 * math.sin(t * 2.6))
    a:new_event()
    a:append("{\\an7\\pos(0,0)\\bord0\\shad0\\1c" .. ACCENT .. "\\1a&H00&}")
    a:draw_start()
    arc_ring(a, cx, cy, 34, 28, spin, spin + len)
    a:draw_stop()
    loading_overlay.data = a.text
    loading_overlay:update()
end

local function loading(on)
    if on then
        if not loading_timer then
            loading_since = mp.get_time()
            loading_timer = mp.add_periodic_timer(1 / 30, draw_loading)
        end
        draw_loading()
    else
        if loading_timer then loading_timer:kill(); loading_timer = nil end
        loading_overlay:remove()
    end
end

loading(true)
mp.register_event("start-file", function() loading(true) end)
mp.register_event("playback-restart", function()
    if not mp.get_property_bool("paused-for-cache", false) then loading(false) end
end)
mp.register_event("end-file", function() loading(false) end)
mp.observe_property("paused-for-cache", "bool", function(_, waiting)
    if waiting then loading(true) elseif mp.get_property("time-pos") then loading(false) end
end)
