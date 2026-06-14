local __lux_exports = {}
local get
local install
local isFill
local isPattern
local keySet
local normalizePaintSlots
local normalizeStyle
local supports

local __lux_import_1 = __lux_import("@lux/mgfx/style")
local style = __lux_import_1
local makeColor = Color
local tableCopy = table.Copy
local typeOf = type
keySet = function(...)
  local out = {  }
  for index = 1, select("#", ...) do
    local key = select(index, ...)
    out[key] = true
  end
  return out
end
local TARGET = {  }
local TARGET_NAME = {  }
local matrix = {  }
TARGET.ROUNDED_BOX = 1
TARGET_NAME[1] = "MGFX.TARGET.ROUNDED_BOX"
matrix[1] = { family = "shape", coverage = "roundedBox", keys = keySet("fill", "stroke", "strokeWidth", "shadow", "outerGlow", "innerGlow", "backdrop", "pattern", "transform") }
TARGET.CHAMFER_BOX = 2
TARGET_NAME[2] = "MGFX.TARGET.CHAMFER_BOX"
matrix[2] = { family = "shape", coverage = "chamferBox", keys = keySet("fill", "stroke", "strokeWidth", "shadow", "outerGlow", "innerGlow", "backdrop", "pattern", "cuts", "transform") }
TARGET.POLY = 3
TARGET_NAME[3] = "MGFX.TARGET.POLY"
matrix[3] = { family = "shape", coverage = "convexPoly", keys = keySet("fill", "stroke", "strokeWidth", "shadow", "backdrop", "pattern", "transform") }
TARGET.LINE = 4
TARGET_NAME[4] = "MGFX.TARGET.LINE"
matrix[4] = { family = "shape", coverage = "line", keys = keySet("fill", "color", "width", "radius", "caps", "noCaps", "backdrop", "transform") }
TARGET.CIRCLE = 5
TARGET_NAME[5] = "MGFX.TARGET.CIRCLE"
matrix[5] = { family = "shape", coverage = "circle", keys = keySet("fill", "stroke", "strokeWidth", "shadow", "outerGlow", "innerGlow", "backdrop", "pattern", "transform") }
TARGET.CAPSULE = 6
TARGET_NAME[6] = "MGFX.TARGET.CAPSULE"
matrix[6] = { family = "shape", coverage = "capsule", keys = keySet("fill", "stroke", "strokeWidth", "shadow", "outerGlow", "innerGlow", "backdrop", "pattern", "transform") }
TARGET.IMAGE = 8
TARGET_NAME[8] = "MGFX.TARGET.IMAGE"
matrix[8] = { family = "content", coverage = "imageMask", keys = keySet("source", "fill", "background", "stroke", "strokeWidth", "shadow", "outerGlow", "backdrop", "mask", "radius", "tint", "color", "alpha", "fit", "objectFit", "position", "positionX", "positionY", "alignX", "alignY", "crop", "uv", "transform") }
TARGET.PROGRESS_BAR = 9
TARGET_NAME[9] = "MGFX.TARGET.PROGRESS_BAR"
matrix[9] = { family = "compound", coverage = "progress", keys = keySet("track", "fill", "stroke", "strokeWidth", "radius", "padding", "outerGlow", "innerGlow", "fillPattern", "trackPattern", "pattern", "fx", "transform") }
TARGET.SEGMENT_BAR = 10
TARGET_NAME[10] = "MGFX.TARGET.SEGMENT_BAR"
matrix[10] = { family = "compound", coverage = "segmentBar", keys = keySet("segments", "gap", "track", "fill", "stroke", "strokeWidth", "radius", "background", "backgroundRadius", "outerGlow", "innerGlow", "fillPattern", "trackPattern", "pattern", "transform") }
TARGET.RING = 11
TARGET_NAME[11] = "MGFX.TARGET.RING"
matrix[11] = { family = "shape", coverage = "ring", keys = keySet("fill", "color", "stroke", "strokeWidth", "outerGlow", "innerGlow", "backdrop", "pattern", "transform") }
TARGET.ARC = 12
TARGET_NAME[12] = "MGFX.TARGET.ARC"
matrix[12] = { family = "shape", coverage = "arc", keys = keySet("fill", "color", "stroke", "strokeWidth", "outerGlow", "innerGlow", "backdrop", "pattern", "transform") }
TARGET.TEXT = 13
TARGET_NAME[13] = "MGFX.TARGET.TEXT"
matrix[13] = { family = "text", coverage = "glyph", keys = keySet("fill", "color", "alignX", "alignY", "valign", "shadow", "stroke", "glow", "weight", "italic", "letterSpacing", "lineHeight") }
TARGET.SECTOR = 14
TARGET_NAME[14] = "MGFX.TARGET.SECTOR"
matrix[14] = { family = "shape", coverage = "sector", keys = keySet("fill", "color", "stroke", "strokeWidth", "outerGlow", "innerGlow", "backdrop", "pattern", "transform") }
get = function(target)
  return matrix[target]
end
supports = function(target, key)
  local entry = get(target)
  return (((entry ~= nil) and (entry.keys ~= nil)) and (entry.keys[key] == true))
end
isPattern = function(value)
  return ((typeOf(value) == "table") and ((((value.kind == style.PATTERN_STRIPE) or (value.kind == style.PATTERN_SMOKE)) or (value.kind == "stripe")) or (value.kind == "smoke")))
end
isFill = function(value)
  if (typeOf(value) ~= "table") then
    return true
  end
  return (((((value.r ~= nil) or (value.kind == style.FILL_SOLID)) or (value.kind == style.FILL_LINEAR)) or (value.kind == style.FILL_RADIAL)) or (value.kind == style.FILL_CONIC))
end
normalizePaintSlots = function(input, target)
  if ((typeOf(input) ~= "table") or not isPattern(input.fill)) then
    return input
  end
  if ((target == TARGET.PROGRESS_BAR) or (target == TARGET.SEGMENT_BAR)) then
    local out = tableCopy(input)
    if (out.fillPattern == nil) then
      out.fillPattern = out.fill
    end
    do
      local __lux_tmp_2 = out.fillBase
      if __lux_tmp_2 == nil then
        __lux_tmp_2 = out.baseFill
      end
      out.fill = __lux_tmp_2
    end
    return out
  end
  local cap = get(target)
  if ((((cap ~= nil) and (cap.keys ~= nil)) and (cap.keys.pattern == true)) and (input.pattern == nil)) then
    local out = tableCopy(input)
    out.pattern = out.fill
    do
      local __lux_tmp_3 = out.patternBase
      if __lux_tmp_3 == nil then
        local __lux_tmp_4 = out.baseFill
        if __lux_tmp_4 == nil then
          __lux_tmp_4 = makeColor(0, 0, 0, 0)
        end
        __lux_tmp_3 = __lux_tmp_4
      end
      out.fill = __lux_tmp_3
    end
    return out
  end
  return input
end
normalizeStyle = function(input, target)
  if target == nil then
    target = nil
  end
  if (input == nil) then
    return {  }
  end
  if style.isColor(input) then
    return { fill = style.solid(input) }
  end
  if (typeOf(input) ~= "table") then
    return {  }
  end
  local out = normalizePaintSlots((tableCopy(input)), target)
  if (out.fill ~= nil) then
    out.fill = style.fillFrom(out.fill)
  end
  if (out.strokeWidth ~= nil) then
    out.strokeWidth = style.strokeWidth((out.strokeWidth), 1)
  end
  if ((out.radius ~= nil) and (target ~= nil)) then
    out.radius = out.radius
  end
  if (out.backdrop ~= nil) then
    out.backdrop = style.backdropStyle(out.backdrop)
  end
  if (out.mask ~= nil) then
    out.mask = style.imageMaskStyle((out.mask), out)
  end
  return out
end
install = function(owner)
  owner.GetCapabilities = get
  owner.Supports = supports
  owner.IsPattern = isPattern
  owner.IsFill = isFill
  owner.NormalizeStyle = normalizeStyle
  owner.hasShaders = function()
    return false
  end
  owner.shaderStatus = function()
    return { ok = false, shaderVersion = nil, reason = "lux/mgfx fallback renderer active" }
  end
  return owner
end

__lux_exports.TARGET = TARGET
__lux_exports.TARGET_NAME = TARGET_NAME
__lux_exports.get = get
__lux_exports.supports = supports
__lux_exports.isPattern = isPattern
__lux_exports.isFill = isFill
__lux_exports.normalizeStyle = normalizeStyle
__lux_exports.install = install

return __lux_exports
