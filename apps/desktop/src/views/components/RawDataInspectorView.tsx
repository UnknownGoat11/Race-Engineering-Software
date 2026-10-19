import type { JSX } from 'react';

export function RawDataInspectorView(): JSX.Element {
  const channels = [
    { id: '0x0001', name: 'SPEED', type: 'f32', val: '60.42 m/s (217.5 km/h)', dq: 'LIVE', hz: 100 },
    { id: '0x0002', name: 'THROTTLE', type: 'f32', val: '0.850 (85.0%)', dq: 'LIVE', hz: 100 },
    { id: '0x0003', name: 'BRAKE', type: 'f32', val: '0.000 (0.0%)', dq: 'LIVE', hz: 100 },
    { id: '0x0004', name: 'CLUTCH', type: 'f32', val: '0.000', dq: 'LIVE', hz: 100 },
    { id: '0x0005', name: 'STEERING_ANGLE', type: 'f32', val: '0.248 rad (14.2°)', dq: 'LIVE', hz: 100 },
    { id: '0x0006', name: 'GEAR', type: 'i8', val: '4', dq: 'LIVE', hz: 100 },
    { id: '0x0007', name: 'RPM', type: 'f32', val: '8240.0', dq: 'LIVE', hz: 100 },
    { id: '0x0010', name: 'ACCEL_LATERAL', type: 'f32', val: '14.22 m/s² (1.45G)', dq: 'LIVE', hz: 100 },
    { id: '0x0011', name: 'ACCEL_LONGITUDINAL', type: 'f32', val: '6.08 m/s² (0.62G)', dq: 'LIVE', hz: 100 },
    { id: '0x0100', name: 'TYRE_TEMP_CORE_FL', type: 'f32', val: '85.20 °C', dq: 'LIVE', hz: 100 },
    { id: '0x0101', name: 'TYRE_TEMP_CORE_FR', type: 'f32', val: '83.10 °C', dq: 'LIVE', hz: 100 },
    { id: '0x0102', name: 'TYRE_TEMP_CORE_RL', type: 'f32', val: '89.00 °C', dq: 'LIVE', hz: 100 },
    { id: '0x0103', name: 'TYRE_TEMP_CORE_RR', type: 'f32', val: '88.40 °C', dq: 'LIVE', hz: 100 },
    { id: '0x0120', name: 'BRAKE_TEMP_FL', type: 'f32', val: '440.0 °C', dq: 'LIVE', hz: 100 },
    { id: '0x0124', name: 'BRAKE_TEMP_PEAK_FL', type: 'f32', val: '560.0 °C', dq: 'LIVE', hz: 100 },
    { id: '0x0200', name: 'FUEL', type: 'f32', val: '42.50 L', dq: 'LIVE', hz: 5 },
    { id: '0x0201', name: 'FUEL_PER_LAP', type: 'f32', val: '2.84 L', dq: 'LIVE', hz: 5 },
    { id: '0x0300', name: 'BATTERY_SOC', type: 'f32', val: '0.680 (68.0%)', dq: 'LIVE', hz: 10 },
    { id: '0x0301', name: 'VIRTUAL_ENERGY', type: 'f32', val: '0.684 (68.4%)', dq: 'LIVE', hz: 10 },
  ];

  return (
    <div className="raw-data-view" data-testid="raw-data-view">
      <div className="raw-header">
        <span className="badge badge-subtle">V1.1 PROTOCOL & WIRE INSPECTOR</span>
        <h2 className="raw-title">~ Raw Data ~ (TLV Channel Bus Inspector)</h2>
        <p className="raw-desc">
          Low-level diagnostic stream monitor displaying live channel IDs, wire value types, 100 Hz sample rates, and data quality states.
        </p>
      </div>

      <div className="raw-stats-bar">
        <div className="raw-stat"><span className="raw-stat-label">STREAM FREQ:</span> <strong className="text-cyan">100.0 Hz</strong></div>
        <div className="raw-stat"><span className="raw-stat-label">WIRE PROTOCOL:</span> <strong>UiBatch v1 (TLV)</strong></div>
        <div className="raw-stat"><span className="raw-stat-label">BATCHES EMITTED:</span> <strong>2,480</strong></div>
        <div className="raw-stat"><span className="raw-stat-label">REJECTED / DROPPED:</span> <strong className="text-green">0 (0.00%)</strong></div>
        <div className="raw-stat"><span className="raw-stat-label">RESYNC REQUIRED:</span> <strong className="text-green">FALSE</strong></div>
      </div>

      <div className="raw-table-card">
        <table className="raw-table">
          <thead>
            <tr>
              <th>Channel ID</th>
              <th>Constant Name</th>
              <th>Type</th>
              <th>Current Decoded Value</th>
              <th>Quality</th>
              <th>Rate</th>
            </tr>
          </thead>
          <tbody>
            {channels.map((ch) => (
              <tr key={ch.id}>
                <td className="font-mono text-cyan">{ch.id}</td>
                <td className="font-bold">{ch.name}</td>
                <td className="font-mono">{ch.type}</td>
                <td className="font-mono text-white">{ch.val}</td>
                <td><span className="status-badge status-badge-ok">{ch.dq}</span></td>
                <td className="font-mono">{ch.hz} Hz</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
