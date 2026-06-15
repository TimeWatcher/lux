return function(__lux_import)
  --#lux source: C:\Development\gmod\lux\examples\gmod_project\src\shared\math.lux:1
  local __lux_exports = {}
  local add
  --#lux source: C:\Development\gmod\lux\examples\gmod_project\src\shared\math.lux:3
  local values
  --#lux source: C:\Development\gmod\lux\examples\gmod_project\src\shared\math.lux:1
  do
    add = function(a, b)
      return a + b
    end
  --#lux source: C:\Development\gmod\lux\examples\gmod_project\src\shared\math.lux:3
    values = function(...)
      return ...
    end
  --#lux source: C:\Development\gmod\lux\examples\gmod_project\src\shared\math.lux:1
  end

  __lux_exports.add = add
  --#lux source: C:\Development\gmod\lux\examples\gmod_project\src\shared\math.lux:3
  __lux_exports.values = values

  --#lux source: C:\Development\gmod\lux\examples\gmod_project\src\shared\math.lux:1
  return __lux_exports
end
