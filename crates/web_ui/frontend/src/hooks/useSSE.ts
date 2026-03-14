import { useEffect, useRef } from 'react';
import { useAppStore } from '../stores/appStore';
import { pushPoint } from '../components/NodeEditor/oscilloscopeBuffer';
import type { InputStatus, FusionStatus } from '../api/types';

export function useSSE() {
  const prevInputRef = useRef<{ status: InputStatus | null; time: number }>({ status: null, time: 0 });
  const prevFusionRef = useRef<{ status: FusionStatus | null; time: number }>({ status: null, time: 0 });
  const throttleRef = useRef<{ fusedPose: number; optical: number; vehiclePose: number }>({ fusedPose: 0, optical: 0, vehiclePose: 0 });

  useEffect(() => {
    const es = new EventSource('/api/events');

    es.onopen = () => {
      useAppStore.getState().setSseConnected(true);
      useAppStore.getState().refreshConfig();
      useAppStore.getState().fetchLicenseStatus();
    };
    es.onerror = () => {
      useAppStore.getState().setSseConnected(false);
    };

    function handle(type: string, data: any) {
      const state = useAppStore.getState();

      switch (type) {
        case 'config':
          state.setConfig(data);
          break;

        case 'status': {
          const now = Date.now();
          const prev = prevFusionRef.current;
          if (prev.status && prev.time) {
            const dt = (now - prev.time) / 1000;
            if (dt > 0.1) {
              const rates = { ...state.fusionRates };
              for (const k of ['nImu', 'nOptical'] as const) {
                const delta = (data[k] || 0) - (prev.status[k] || 0);
                rates[k] = delta > 0 ? Math.round(delta / dt) : 0;
              }
              state.setFusionRates(rates);
            }
          }
          prevFusionRef.current = { status: { ...data }, time: now };
          state.setStatus(data);
          break;
        }

        case 'inputStatus': {
          const now = Date.now();
          const prev = prevInputRef.current;
          if (prev.status && prev.time) {
            const dt = (now - prev.time) / 1000;
            if (dt > 0.1) {
              const rates = { ...state.inputRates };
              for (const k of ['nImu', 'nOptical', 'nGnss', 'nCan', 'nRtcmData', 'nVehicleSpeed'] as const) {
                const delta = (data[k] || 0) - ((prev.status as any)[k] || 0);
                (rates as any)[k] = delta > 0 ? Math.round(delta / dt) : 0;
              }
              state.setInputRates(rates);
            }
          }
          prevInputRef.current = { status: { ...data }, time: now };
          state.setInputStatus(data);
          break;
        }

        case 'getIntercalibrationStatus':
        case 'intercalibrationResult':
          state.setIntercalibrationStatus(data);
          break;

        case 'licenseStatus': {
          const info = data.info || data;
          state.setLicenseInfo(info);
          if (data.licenseKey) state.setLicenseField('licenseKey', data.licenseKey);
          if (data.serverUrl) state.setLicenseField('serverUrl', data.serverUrl);
          break;
        }

        case 'fusedPose': {
          const now = Date.now();
          if (now - throttleRef.current.fusedPose < 50) break;
          throttleRef.current.fusedPose = now;
          const pose = data.fusedPose || data;
          state.setFusedPose(pose);
          break;
        }

        case 'opticalData': {
          const now = Date.now();
          if (now - throttleRef.current.optical < 50) break;
          throttleRef.current.optical = now;
          const opt = data.opticalData || data;
          state.setOpticalData(opt);
          break;
        }

        case 'fusedVehiclePose': {
          const now = Date.now();
          if (now - throttleRef.current.vehiclePose < 50) break;
          throttleRef.current.vehiclePose = now;
          const vp = data.fusedVehiclePose || data.fusedVehiclePoseV2 || data;
          state.setFusedVehiclePose(vp);
          break;
        }

        case 'nodeStatuses':
          state.setNodeStatuses(data);
          break;

        case 'mcpStatus':
          state.setMcpStatus(data);
          break;

        case 'aiMonitorStatus':
          state.setAiMonitorStatus(data);
          break;

        case 'log':
          if (Array.isArray(data)) {
            state.addLogEntries(data);
          }
          break;

        case 'oscilloscope':
          pushPoint(data.t, data.v);
          break;

        case 'oscilloscopeTypes':
          state.setOscilloscopeTypes(data.types);
          break;
      }
    }

    const eventTypes = [
      'config', 'status', 'inputStatus',
      'getIntercalibrationStatus', 'intercalibrationResult',
      'licenseStatus', 'fusedPose', 'opticalData', 'fusedVehiclePose',
      'nodeStatuses', 'mcpStatus', 'aiMonitorStatus', 'log', 'oscilloscope',
      'oscilloscopeTypes',
    ];

    for (const type of eventTypes) {
      es.addEventListener(type, ((e: MessageEvent) => {
        try {
          const data = JSON.parse(e.data);
          handle(type, data);
        } catch {}
      }) as EventListener);
    }

    // Fallback: handle unnamed messages with embedded type
    es.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type && msg.data) {
          handle(msg.type, msg.data);
        }
      } catch {}
    };

    return () => {
      es.close();
      useAppStore.getState().setSseConnected(false);
    };
  }, []);
}
