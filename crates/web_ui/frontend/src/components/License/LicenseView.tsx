import { useEffect, useRef } from 'react';
import { useAppStore } from '../../stores/appStore';

function licenseStatusText(status: string): string {
  const map: Record<string, string> = {
    valid: 'Valid',
    grace_period: 'Valid (Grace Period)',
    expired: 'Expired',
    lease_expired: 'Lease Expired',
    invalid_machine: 'Invalid Machine',
    invalid_signature: 'Invalid Signature',
    file_not_found: 'Not Found',
    token_not_found: 'Token Not Found',
    error: 'Error',
    not_checked: 'Not Checked',
  };
  return map[status] || status;
}

function statusBadgeClass(status: string): string {
  if (status === 'valid') return 'connected';
  if (status === 'grace_period') return 'connecting';
  return 'disconnected';
}

function formatExpiry(iso: string | null): string {
  if (!iso) return 'Perpetual';
  try { return new Date(iso).toLocaleDateString(); } catch { return iso; }
}

export default function LicenseView() {
  const license = useAppStore((s) => s.license);
  const setLicenseField = useAppStore((s) => s.setLicenseField);
  const checkLicenseFile = useAppStore((s) => s.checkLicenseFile);
  const checkLicenseServer = useAppStore((s) => s.checkLicenseServer);
  const checkLicenseToken = useAppStore((s) => s.checkLicenseToken);
  const uploadLicense = useAppStore((s) => s.uploadLicense);
  const fetchMachines = useAppStore((s) => s.fetchMachines);
  const deactivateMachine = useAppStore((s) => s.deactivateMachine);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Auto-fetch machines when license is valid and a key is available
  useEffect(() => {
    if (license.info.valid && license.licenseKey && license.serverUrl && license.machines.length === 0 && !license.machinesLoading) {
      fetchMachines();
    }
  }, [license.info.valid, license.licenseKey, license.serverUrl, license.machines.length, license.machinesLoading, fetchMachines]);

  const handleUpload = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    await uploadLicense(file);
    e.target.value = '';
  };

  return (
    <div className="view-container">
      <div className="card">
        <div className="card-title">License Status</div>
        <div className="card-divider" />
        <div className="card-body">
          <table>
            <tbody>
              <tr><td>Status</td><td><span className={`badge ${statusBadgeClass(license.info.status)}`}>{licenseStatusText(license.info.status)}</span></td></tr>
              <tr><td>Customer</td><td>{license.info.customer || '-'}</td></tr>
              <tr><td>Product</td><td>{license.info.product || '-'}</td></tr>
              <tr><td>Features</td><td>{license.info.features?.join(', ') || '-'}</td></tr>
              <tr><td>Expires</td><td>{formatExpiry(license.info.expires)}</td></tr>
              <tr><td>Lease Expires</td><td>{formatExpiry(license.info.lease_expires)}</td></tr>
              <tr><td>Machine Code</td><td style={{ fontFamily: 'monospace', fontSize: 11 }}>{license.info.machine_code || '-'}</td></tr>
              {license.licenseKey && <tr><td>License Key</td><td style={{ fontFamily: 'monospace', fontSize: 11 }}>{license.licenseKey}</td></tr>}
              {license.info.error && <tr><td>Error</td><td className="error-text">{license.info.error}</td></tr>}
            </tbody>
          </table>
        </div>
      </div>

      <div className="card">
        <div className="card-title">Activate License</div>
        <div className="card-divider" />
        <div className="card-body flex-col gap-16">
          <div className="flex-row">
            <button className={license.method === 'file' ? '' : 'secondary'} onClick={() => setLicenseField('method', 'file')}>License File</button>
            <button className={license.method === 'server' ? '' : 'secondary'} onClick={() => setLicenseField('method', 'server')}>License Server</button>
            <button className={license.method === 'token' ? '' : 'secondary'} onClick={() => setLicenseField('method', 'token')}>USB Token</button>
          </div>

          {license.method === 'file' && (
            <div className="flex-col gap-8">
              <div>
                <h3>License File Path</h3>
                <input type="text" value={license.licenseFile} onChange={(e) => setLicenseField('licenseFile', e.target.value)} />
              </div>
              <div className="flex-row">
                <button onClick={checkLicenseFile} disabled={license.loading}>Check File</button>
                <button className="secondary" onClick={() => fileInputRef.current?.click()} disabled={license.loading}>Upload File</button>
                <input ref={fileInputRef} type="file" accept=".json" style={{ display: 'none' }} onChange={handleUpload} />
              </div>
            </div>
          )}

          {license.method === 'server' && (
            <div className="flex-col gap-8">
              <div>
                <h3>License Key</h3>
                <input type="text" value={license.licenseKey} onChange={(e) => setLicenseField('licenseKey', e.target.value)} placeholder="Enter license key" />
              </div>
              <div>
                <h3>Server URL</h3>
                <input type="text" value={license.serverUrl} onChange={(e) => setLicenseField('serverUrl', e.target.value)} />
              </div>
              <div className="flex-row">
                <button onClick={checkLicenseServer} disabled={license.loading}>Activate</button>
                <button className="secondary" onClick={fetchMachines} disabled={license.machinesLoading || !license.licenseKey}>View Machines</button>
              </div>
            </div>
          )}

          {license.method === 'token' && (
            <div className="flex-col gap-8">
              <p style={{ color: 'var(--text2)', fontSize: 13 }}>Check for a connected USB license token.</p>
              <button onClick={checkLicenseToken} disabled={license.loading}>Check USB Token</button>
            </div>
          )}

          {license.message && (
            <div className={`toast ${license.messageType}`} style={{ position: 'relative', left: 0, bottom: 0, transform: 'none' }}>
              {license.message}
            </div>
          )}
        </div>
      </div>

      {license.machines.length > 0 && (
        <div className="card">
          <div className="card-title">Active Machines ({license.machines.length}/{license.machinesMax})</div>
          <div className="card-divider" />
          <div className="card-body">
            {license.machinesError && <div className="error-text mb-8">{license.machinesError}</div>}
            <table>
              <thead>
                <tr>
                  <td style={{ fontWeight: 600 }}>Machine Code</td>
                  <td style={{ fontWeight: 600 }}>Last Seen</td>
                  <td style={{ fontWeight: 600 }}>Actions</td>
                </tr>
              </thead>
              <tbody>
                {license.machines.map((m: any, i: number) => (
                  <tr key={i}>
                    <td style={{ fontFamily: 'monospace', fontSize: 11 }}>{m.machine_code}</td>
                    <td>{m.last_seen ? new Date(m.last_seen).toLocaleString() : '-'}</td>
                    <td>
                      <button
                        className="danger"
                        style={{ padding: '4px 8px', fontSize: 12 }}
                        onClick={() => {
                          const isSelf = m.machine_code === license.info.machine_code;
                          const msg = isSelf
                            ? 'Remove THIS machine from the license? The current license will be invalidated and you will need to re-activate.'
                            : 'Remove this machine from the license? It will need to re-activate to use the license again.';
                          if (confirm(msg)) deactivateMachine(m.machine_code);
                        }}
                        disabled={license.machinesLoading}
                      >
                        Remove
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}
