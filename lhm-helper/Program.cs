// BugJuice LHM helper — reads power sensors via LibreHardwareMonitorLib
// and writes JSON lines to stdout for the Rust service to consume.
//
// Runs as a long-lived child process of bugjuice-svc.exe (SYSTEM).
// Output format matches the EmiReading wire format so the Tauri app
// consumes it without changes.

using System.Text.Json;
using LibreHardwareMonitor.Hardware;

var computer = new Computer
{
    IsCpuEnabled = true,
    IsGpuEnabled = true,
    IsMemoryEnabled = true,
};

try
{
    computer.Open();
}
catch (Exception ex)
{
    Console.Error.WriteLine($"[bugjuice-lhm] Failed to open hardware: {ex.Message}");
    Environment.Exit(1);
}

Console.Error.WriteLine("[bugjuice-lhm] Started — reading power sensors");

var visitor = new UpdateVisitor();
var jsonOptions = new JsonSerializerOptions { PropertyNamingPolicy = JsonNamingPolicy.CamelCase };

while (true)
{
    try
    {
        computer.Accept(visitor);

        var channels = new List<ChannelDto>();

        foreach (var hw in computer.Hardware)
        {
            CollectPowerSensors(hw, channels);
        }

        if (channels.Count > 0)
        {
            var reading = new ReadingDto
            {
                Version = 0,
                Oem = "LHM",
                Model = "LibreHardwareMonitor",
                Channels = channels,
            };

            var json = JsonSerializer.Serialize(reading, jsonOptions);
            Console.WriteLine(json);
            Console.Out.Flush();
        }
    }
    catch (Exception ex)
    {
        Console.Error.WriteLine($"[bugjuice-lhm] Poll error: {ex.Message}");
    }

    Thread.Sleep(2000);
}

static void CollectPowerSensors(IHardware hw, List<ChannelDto> channels)
{
    foreach (var sensor in hw.Sensors)
    {
        if (sensor.SensorType != SensorType.Power || !sensor.Value.HasValue)
            continue;

        var watts = (double)sensor.Value.Value;
        if (!double.IsFinite(watts) || watts < 0)
            continue;

        var name = MapSensorName(sensor);
        if (name != null)
        {
            channels.Add(new ChannelDto { Name = name, Watts = watts });
        }
    }

    // Recurse into sub-hardware (e.g. GPU sub-devices)
    foreach (var sub in hw.SubHardware)
    {
        CollectPowerSensors(sub, channels);
    }
}

/// Map LHM sensor names to EMI-compatible channel names so the Tauri
/// app's existing channel-matching logic picks them up automatically.
static string? MapSensorName(ISensor sensor)
{
    var name = sensor.Name.ToLowerInvariant();

    // CPU Package (Intel PKG / AMD Package)
    if (name.Contains("cpu package") || name == "package")
        return "RAPL_Package0_PKG";

    // CPU Cores (Intel PP0 / AMD sum-of-cores)
    if (name.Contains("cpu cores") || name == "cores")
        return "RAPL_Package0_PP0";

    // GPU power (discrete or integrated)
    if (name.Contains("gpu power") || name.Contains("gpu package"))
        return "GPU";

    // DRAM / Memory
    if (name.Contains("dram") || name.Contains("memory"))
        return "RAPL_Package0_DRAM";

    // Skip per-core entries ("CPU Core #1 Power" etc.) and unknown sensors
    return null;
}

// ─── DTOs matching the EmiReading wire format ────────────────────────────

record ReadingDto
{
    public required ushort Version { get; init; }
    public required string Oem { get; init; }
    public required string Model { get; init; }
    public required List<ChannelDto> Channels { get; init; }
}

record ChannelDto
{
    public required string Name { get; init; }
    public required double Watts { get; init; }
}

// ─── IVisitor to refresh all hardware sensors ────────────────────────────

class UpdateVisitor : IVisitor
{
    public void VisitComputer(IComputer computer) => computer.Traverse(this);

    public void VisitHardware(IHardware hardware)
    {
        hardware.Update();
        foreach (var sub in hardware.SubHardware)
            sub.Accept(this);
    }

    public void VisitSensor(ISensor sensor) { }
    public void VisitParameter(IParameter parameter) { }
}
