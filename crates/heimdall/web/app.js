/**
 * HEIMDALL // Bifrost BBS Master Supervisor Client Engine
 */

class HeimdallApp {
  constructor() {
    this.activeTab = 'overview';
    this.logsWs = null;
    this.termWs = null;
    this.logs = [];
    this.logIdSet = new Set();
    this.autoScrollLogs = true;
    this.currentSelectedApp = null;
    this.currentEditingFile = null;
    this.currentConfig = null;
    this.currentSelectedTable = null;
    
    this.init();
  }

  init() {
    this.bindEvents();
    this.initTheme();
    this.startClock();
    
    // Initial fetches
    this.fetchOverview();
    this.fetchServices();
    this.fetchApps();
    this.fetchConfig();
    this.fetchTelemetry();
    this.fetchCaptures();
    this.fetchDatabase();
    this.fetchHistoricalLogs();

    // Connect Log Stream WebSocket
    this.connectLogsWebSocket();

    // Continuous Realtime Polling Interval (every 3 seconds)
    setInterval(() => {
      this.fetchOverview();
      this.fetchServices();
      if (this.activeTab === 'telemetry') {
        this.fetchTelemetry();
        this.fetchCaptures();
      }
      if (this.activeTab === 'database') {
        this.fetchDatabase();
      }
    }, 3000);
  }

  bindEvents() {
    const on = (id, event, fn) => {
      const el = document.getElementById(id);
      if (el) el.addEventListener(event, fn);
    };

    // Nav Tabs
    document.querySelectorAll('.nav-tab').forEach(tabBtn => {
      tabBtn.addEventListener('click', () => {
        this.switchTab(tabBtn.dataset.tab);
      });
    });

    // Theme & Scanlines
    on('theme-selector', 'change', (e) => {
      this.setTheme(e.target.value);
    });

    on('toggle-scanlines', 'click', () => {
      const isOverlay = document.body.classList.toggle('crt-overlay');
      const btn = document.getElementById('toggle-scanlines');
      if (btn) {
        btn.textContent = `CRT FX: ${isOverlay ? 'ON' : 'OFF'}`;
        btn.setAttribute('aria-pressed', isOverlay);
      }
    });

    // Supervisor Quick Actions
    on('btn-start-bbs', 'click', () => this.callSupervisorAction('start_bbs'));
    on('btn-stop-bbs', 'click', () => this.callSupervisorAction('stop_bbs'));
    on('btn-restart-bbs', 'click', () => this.callSupervisorAction('restart_bbs'));
    on('btn-quick-crawler', 'click', () => this.startCrawler(100, 50));
    on('btn-quick-benchmark', 'click', () => this.runTuning('analyze', []));

    // Logs Controls
    on('log-level-filter', 'change', () => this.renderLogs());
    on('log-source-filter', 'change', () => this.renderLogs());
    on('log-search-input', 'input', () => this.renderLogs());
    on('btn-refresh-logs', 'click', () => this.fetchHistoricalLogs());
    on('btn-toggle-tail', 'click', () => {
      this.autoScrollLogs = !this.autoScrollLogs;
      const btn = document.getElementById('btn-toggle-tail');
      if (btn) {
        btn.textContent = `AUTO-SCROLL: ${this.autoScrollLogs ? 'ON' : 'OFF'}`;
        btn.classList.toggle('active', this.autoScrollLogs);
        btn.setAttribute('aria-pressed', this.autoScrollLogs);
      }
    });
    on('btn-clear-logs', 'click', () => {
      this.logs = [];
      this.logIdSet.clear();
      this.renderLogs();
    });

    // Apps Refresh
    on('btn-refresh-apps', 'click', () => this.fetchApps());

    // Database Controls
    on('btn-refresh-db', 'click', () => this.fetchDatabase());
    on('btn-clear-table', 'click', () => this.clearCurrentTable());
    on('btn-save-key', 'click', () => this.saveCurrentKey());

    // DB Backup / Restore / Reset
    on('btn-backup-db', 'click', () => {
      window.location.href = '/api/database/backup';
    });

    const restoreInput = document.getElementById('db-restore-file-input');
    on('btn-restore-db', 'click', () => {
      if (restoreInput) restoreInput.click();
    });
    if (restoreInput) {
      restoreInput.addEventListener('change', async (e) => {
        const file = e.target.files[0];
        if (!file) return;
        if (!confirm(`Restore database from file "${file.name}"? This will overwrite the active database.`)) {
          restoreInput.value = '';
          return;
        }
        try {
          const buffer = await file.arrayBuffer();
          const res = await fetch('/api/database/restore', {
            method: 'POST',
            headers: { 'Content-Type': 'application/octet-stream' },
            body: buffer,
          });
          if (res.ok) {
            alert('Database restored successfully!');
            this.fetchDatabase();
            this.fetchOverview();
          } else {
            alert('Failed to restore database: ' + await res.text());
          }
        } catch (err) {
          alert('Error restoring database: ' + err);
        } finally {
          restoreInput.value = '';
        }
      });
    }

    const nukeModal = document.getElementById('modal-reset-db');
    const nukeInput = document.getElementById('input-nuke-confirm');
    const nukeConfirmBtn = document.getElementById('btn-confirm-nuke');
    const nukeCancelBtn = document.getElementById('btn-cancel-nuke');

    on('btn-reset-db', 'click', () => {
      if (!confirm('RESET DATABASE (Step 1 of 2): Are you sure you want to reset the database? All tables will be wiped.')) {
        return;
      }
      if (nukeInput && nukeConfirmBtn && nukeModal) {
        nukeInput.value = '';
        nukeConfirmBtn.disabled = true;
        nukeModal.style.display = 'flex';
        nukeInput.focus();
      }
    });

    if (nukeInput && nukeConfirmBtn) {
      nukeInput.addEventListener('input', () => {
        nukeConfirmBtn.disabled = nukeInput.value.trim().toUpperCase() !== 'NUKE IT';
      });
    }

    if (nukeCancelBtn && nukeModal) {
      nukeCancelBtn.addEventListener('click', () => {
        nukeModal.style.display = 'none';
        if (nukeInput) nukeInput.value = '';
      });
    }

    if (nukeConfirmBtn) {
      nukeConfirmBtn.addEventListener('click', async () => {
        try {
          const res = await fetch('/api/database/reset', { method: 'POST' });
          if (res.ok) {
            alert('Database has been completely reset and vacuumed.');
            if (nukeModal) nukeModal.style.display = 'none';
            this.fetchDatabase();
            this.fetchOverview();
          } else {
            alert('Failed to reset database: ' + await res.text());
          }
        } catch (err) {
          alert('Error resetting database: ' + err);
        }
      });
    }

    // Web Terminal Controls & Keyboard capture
    on('btn-term-reconnect', 'click', () => this.connectTerminalWebSocket());
    on('btn-term-reset', 'click', () => {
      if (this.termWs && this.termWs.readyState === WebSocket.OPEN) {
        this.termWs.send(JSON.stringify({ type: 'reset' }));
      }
    });

    document.querySelectorAll('.key-btn').forEach(btn => {
      btn.addEventListener('click', () => {
        const key = btn.dataset.key;
        this.sendTerminalKey(key);
      });
    });

    window.addEventListener('keydown', (e) => {
      if (this.activeTab === 'terminal') {
        if (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.tagName === 'SELECT') {
          return; // Don't intercept when user is typing in config/app editor
        }
        
        // Prevent default browser actions for navigation/terminal keys
        if (['Tab', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Backspace', 'Enter', ' '].includes(e.key)) {
          e.preventDefault();
        }

        this.sendTerminalKey(e.key);
      }
    });

    // App Editor
    on('btn-save-app-file', 'click', () => this.saveCurrentAppFile());

    // Config Forms
    on('config-form', 'submit', (e) => {
      e.preventDefault();
      this.saveConfigForm();
    });

    on('btn-reload-config', 'click', () => this.fetchConfig());
    on('btn-save-raw-toml', 'click', () => this.saveRawToml());

    // Tuning Console Actions
    on('btn-run-analyze', 'click', () => this.runTuning('analyze', []));
    on('btn-run-sweep', 'click', () => this.runTuning('sweep', []));
    on('btn-run-train', 'click', () => {
      const tokensEl = document.getElementById('tuning-tokens-input');
      const tokens = tokensEl ? tokensEl.value : '254';
      this.runTuning('train', ['--tokens', tokens]);
    });
    on('btn-run-crawler-custom', 'click', () => {
      const stepsEl = document.getElementById('crawler-steps-input');
      const steps = stepsEl ? parseInt(stepsEl.value, 10) || 100 : 100;
      this.startCrawler(steps, 50);
    });

    on('btn-refresh-captures', 'click', () => this.fetchCaptures());
  }
  }

  switchTab(tabId) {
    this.activeTab = tabId;

    document.querySelectorAll('.nav-tab').forEach(btn => {
      const isActive = btn.dataset.tab === tabId;
      btn.classList.toggle('active', isActive);
      btn.setAttribute('aria-selected', isActive);
    });

    document.querySelectorAll('.tab-pane').forEach(pane => {
      pane.classList.toggle('active', pane.id === `tab-${tabId}`);
    });

    if (tabId === 'terminal') {
      if (!this.termWs || this.termWs.readyState !== WebSocket.OPEN) {
        this.connectTerminalWebSocket();
      }
      setTimeout(() => {
        const screen = document.getElementById('terminal-screen');
        if (screen) screen.focus();
      }, 100);
    } else if (tabId === 'logs') {
      this.fetchHistoricalLogs();
    }
  }

  setTheme(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('heimdall_theme', theme);
  }

  initTheme() {
    const saved = localStorage.getItem('heimdall_theme') || 'green';
    document.getElementById('theme-selector').value = saved;
    this.setTheme(saved);
  }

  startClock() {
    const update = () => {
      const now = new Date();
      document.getElementById('footer-clock').textContent = `${now.toISOString().slice(11, 19)} UTC`;
    };
    update();
    setInterval(update, 1000);
  }

  // --- API CALLS ---

  async fetchOverview() {
    try {
      const res = await fetch('/api/supervisor/status');
      if (!res.ok) return;
      const data = await res.json();
      
      const bbs = data.find(s => s.name === 'bifrost-bbs');
      const dot = document.getElementById('bbs-indicator');
      const text = document.getElementById('bbs-status-text');

      if (dot) {
        dot.className = (bbs && bbs.state === 'running') ? 'indicator-dot active' : 'indicator-dot stopped';
      }
      if (text) {
        if (bbs && bbs.state === 'running') {
          text.textContent = `ONLINE (PID: ${bbs.pid || 'N/A'})`;
        } else {
          text.textContent = bbs ? bbs.state.toUpperCase() : 'OFFLINE';
        }
      }

      // Also refresh telemetry overview cards
      this.fetchTelemetry();
    } catch (e) {
      console.warn('Overview fetch error:', e);
    }
  }

  async fetchServices() {
    try {
      const res = await fetch('/api/supervisor/status');
      if (!res.ok) return;
      const services = await res.json();
      
      const tbody = document.getElementById('services-tbody');
      tbody.innerHTML = '';

      services.forEach(svc => {
        const tr = document.createElement('tr');
        const isRunning = svc.state === 'running';
        tr.innerHTML = `
          <td><strong>${svc.name}</strong></td>
          <td><span class="status-pill ${isRunning ? 'btn-success' : 'btn-danger'}">${svc.state.toUpperCase()}</span></td>
          <td>${svc.pid || '-'}</td>
          <td>${svc.uptime_secs}s</td>
          <td>${svc.restart_count}</td>
          <td>
            ${svc.name === 'bifrost-bbs' ? `
              <button class="retro-btn btn-sm" onclick="app.callSupervisorAction('${isRunning ? 'stop_bbs' : 'start_bbs'}')">
                ${isRunning ? 'STOP' : 'START'}
              </button>
            ` : '-'}
          </td>
        `;
        tbody.appendChild(tr);
      });
    } catch (e) {
      console.warn('Services fetch error:', e);
    }
  }

  async fetchApps() {
    try {
      const res = await fetch('/api/apps');
      if (!res.ok) return;
      const apps = await res.json();
      
      const grid = document.getElementById('apps-grid');
      grid.innerHTML = '';

      apps.forEach(a => {
        const card = document.createElement('div');
        card.className = `app-card ${this.currentSelectedApp === a.id ? 'active' : ''}`;
        card.id = `app-card-${a.id}`;
        card.innerHTML = `
          <strong>${a.name}</strong>
          <span class="app-badge">${a.id}</span>
          <span class="card-sub">v${a.version} | ${a.asset_count} assets</span>
          <p class="card-sub">${a.description || 'No description'}</p>
        `;
        card.addEventListener('click', () => this.selectApp(a.id));
        grid.appendChild(card);
      });

      if (!this.currentSelectedApp && apps.length > 0) {
        this.selectApp(apps[0].id);
      }
    } catch (e) {
      console.warn('Apps fetch error:', e);
    }
  }

  async selectApp(appId) {
    this.currentSelectedApp = appId;
    document.querySelectorAll('.app-card').forEach(c => c.classList.remove('active'));
    const activeCard = document.getElementById(`app-card-${appId}`);
    if (activeCard) activeCard.classList.add('active');

    document.getElementById('app-files-title').textContent = `══ ${appId.toUpperCase()} FILES ══`;
    await this.fetchAppFiles(appId);
  }

  async fetchAppFiles(appId) {
    try {
      const res = await fetch(`/api/apps/${appId}/files`);
      if (!res.ok) return;
      const files = await res.json();

      const listContainer = document.getElementById('app-files-list');
      listContainer.innerHTML = '';

      const nonDirFiles = files.filter(f => !f.is_dir);

      nonDirFiles.forEach(f => {
        const item = document.createElement('div');
        item.className = `app-file-item ${this.currentEditingFile === f.path ? 'active' : ''}`;
        item.id = `file-item-${encodeURIComponent(f.path)}`;
        
        let icon = '[FILE]';
        if (f.name.endsWith('.lua')) icon = '[LUA]';
        else if (f.name.endsWith('.toml')) icon = '[TOML]';
        else if (f.name.endsWith('.ans')) icon = '[ANS]';

        item.innerHTML = `
          <span><span class="file-icon">${icon}</span> ${f.path}</span>
          <span class="file-size">${f.size_bytes} B</span>
        `;
        item.addEventListener('click', () => this.loadFileContent(appId, f.path));
        listContainer.appendChild(item);
      });

      // Default load main.lua or manifest.toml
      const defaultFile = nonDirFiles.find(f => f.path === 'main.lua') || nonDirFiles[0];
      if (defaultFile) {
        this.loadFileContent(appId, defaultFile.path);
      }
    } catch (e) {
      console.warn('Fetch app files error:', e);
    }
  }

  async loadFileContent(appId, filePath) {
    this.currentEditingFile = filePath;
    document.querySelectorAll('.app-file-item').forEach(el => el.classList.remove('active'));
    const activeItem = document.getElementById(`file-item-${encodeURIComponent(filePath)}`);
    if (activeItem) activeItem.classList.add('active');

    document.getElementById('editor-title').textContent = `══ ${appId}/${filePath} ══`;
    document.getElementById('editor-file-badge').textContent = filePath;

    try {
      const res = await fetch(`/api/apps/${appId}/file_content?path=${encodeURIComponent(filePath)}`);
      if (res.ok) {
        const text = await res.text();
        const editor = document.getElementById('app-file-editor');
        editor.value = text;
        document.getElementById('btn-save-app-file').disabled = false;
      }
    } catch (e) {
      console.warn('Load file content error:', e);
    }
  }

  async saveCurrentAppFile() {
    if (!this.currentSelectedApp || !this.currentEditingFile) return;
    const content = document.getElementById('app-file-editor').value;

    try {
      const res = await fetch(`/api/apps/${this.currentSelectedApp}/file_content?path=${encodeURIComponent(this.currentEditingFile)}`, {
        method: 'POST',
        headers: { 'Content-Type': 'text/plain' },
        body: content,
      });
      if (res.ok) {
        alert(`Saved '${this.currentEditingFile}' successfully!`);
        this.fetchAppFiles(this.currentSelectedApp);
      } else {
        alert('Failed to save file: ' + await res.text());
      }
    } catch (e) {
      alert('Error saving file: ' + e);
    }
  }

  async fetchConfig() {
    try {
      const res = await fetch('/api/config');
      if (!res.ok) return;
      const cfg = await res.json();
      this.currentConfig = cfg.parsed;

      // Populate GUI Form Controls based on real AppConfig schema
      if (cfg.parsed) {
        if (cfg.parsed.rate_limiter) {
          document.getElementById('cfg-max-ppm').value = cfg.parsed.rate_limiter.max_packets_per_minute || 45;
          document.getElementById('cfg-max-burst').value = cfg.parsed.rate_limiter.max_burst_packets || 4;
          document.getElementById('cfg-inter-guard').value = cfg.parsed.rate_limiter.inter_packet_guard_ms || 350;
          document.getElementById('cfg-duty-cycle').value = cfg.parsed.rate_limiter.max_duty_cycle_percent || 1.0;
          document.getElementById('cfg-duty-window').value = cfg.parsed.rate_limiter.duty_cycle_window_secs || 3600;
        }

        if (cfg.parsed.asset_broadcaster) {
          document.getElementById('cfg-broadcast-enabled').checked = !!cfg.parsed.asset_broadcaster.enable_on_demand_broadcast;
          document.getElementById('cfg-broadcast-duty').value = cfg.parsed.asset_broadcaster.max_asset_broadcast_duty_cycle || 0.15;
        }

        if (cfg.parsed.apps) {
          document.getElementById('cfg-main-app').value = cfg.parsed.apps.main_app || 'main_menu';
        }

        if (cfg.parsed.packet_capture) {
          document.getElementById('cfg-capture-enabled').checked = !!cfg.parsed.packet_capture.enabled;
          document.getElementById('cfg-capture-dir').value = cfg.parsed.packet_capture.directory || 'captured_packets';
        }

        document.getElementById('cfg-log-level').value = cfg.parsed.log_level || 'info';
      }

      // Populate Raw TOML Editor
      document.getElementById('raw-toml-editor').value = cfg.raw_toml || '';
    } catch (e) {
      console.warn('Config fetch error:', e);
    }
  }

  async saveConfigForm() {
    const rawEditor = document.getElementById('raw-toml-editor');
    
    // Construct valid TOML matching AppConfig
    const tomlStr = `# Bifrost MeshBBS Host Configuration

log_level = "${document.getElementById('cfg-log-level').value}"

[rate_limiter]
max_packets_per_minute = ${parseInt(document.getElementById('cfg-max-ppm').value, 10) || 45}
max_burst_packets = ${parseInt(document.getElementById('cfg-max-burst').value, 10) || 4}
inter_packet_guard_ms = ${parseInt(document.getElementById('cfg-inter-guard').value, 10) || 350}
max_duty_cycle_percent = ${parseFloat(document.getElementById('cfg-duty-cycle').value) || 1.0}
duty_cycle_window_secs = ${parseInt(document.getElementById('cfg-duty-window').value, 10) || 3600}

[asset_broadcaster]
enable_on_demand_broadcast = ${document.getElementById('cfg-broadcast-enabled').checked}
max_asset_broadcast_duty_cycle = ${parseFloat(document.getElementById('cfg-broadcast-duty').value) || 0.15}

[form_colors]
submit_fg = 14
submit_bg = 4
field_fg = 0
field_bg = 14

admin_nodes = [
    "0101010101010101010101010101010101010101010101010101010101010101"
]

[apps]
main_app = "${document.getElementById('cfg-main-app').value}"
enabled = [
    "main_menu",
    "messages",
    "profile",
    "minidungeon",
    "admin",
    "marketplace",
]

[packet_capture]
enabled = ${document.getElementById('cfg-capture-enabled').checked}
directory = "${document.getElementById('cfg-capture-dir').value}"
`;

    rawEditor.value = tomlStr;
    await this.saveRawToml();
  }

  async saveRawToml() {
    const content = document.getElementById('raw-toml-editor').value;
    try {
      const res = await fetch('/api/config', {
        method: 'POST',
        headers: { 'Content-Type': 'text/plain' },
        body: content,
      });
      if (res.ok) {
        alert('Configuration saved! Restarting BBS server...');
        await this.callSupervisorAction('restart_bbs');
        this.fetchConfig();
      } else {
        alert('Failed to save config: ' + await res.text());
      }
    } catch (e) {
      alert('Error saving config: ' + e);
    }
  }

  async fetchTelemetry() {
    try {
      const res = await fetch('/api/telemetry/summary');
      if (!res.ok) return;
      const s = await res.json();

      const setText = (id, val) => {
        const el = document.getElementById(id);
        if (el) el.textContent = val;
      };

      setText('stat-active-sessions', s.active_sessions || '0');
      setText('stat-unique-users', `24h Users: ${s.unique_users_24h || 0}`);
      setText('stat-duty-cycle', `${(s.duty_cycle_percent || 0).toFixed(2)}%`);
      setText('duty-cycle-text', `${(s.duty_cycle_percent || 0).toFixed(2)}% / 1.0%`);
      setText('stat-packets-tx-rx', `${s.total_packets_sent || 0} / ${s.total_packets_received || 0}`);
      setText('stat-ppm', `PPM: ${(s.send_ppm_1h || 0).toFixed(1)} TX / ${(s.recv_ppm_1h || 0).toFixed(1)} RX`);
      setText('stat-compression-savings', `+${(s.compression_savings_percent || 0).toFixed(1)}%`);
      setText('stat-raw-comp-bytes', `Raw: ${s.total_raw_bytes_sent || 0} B | Comp: ${s.total_compressed_bytes_sent || 0} B`);

      // Surface DB numbers to Overview
      if (s.database_summary) {
        const dbSum = s.database_summary;
        const dbTel = s.database || dbSum.telemetry || {};
        setText('stat-db-size', formatBytes(dbSum.size_bytes || 0));
        setText('stat-db-sub', `Records: ${(dbSum.total_records || 0).toLocaleString()} | Growth: ${formatBytes(dbTel.byte_growth_per_day || 0)}/d`);
      }

      // Populate Database Telemetry & Stats grid
      if (s.database) {
        const dt = s.database;
        setText('db-stat-qph', (dt.queries_per_hour || 0).toFixed(1));
        setText('db-stat-total-queries', `Total: ${(dt.total_queries || 0).toLocaleString()} queries`);
        setText('db-stat-avg-time', `${(dt.avg_query_time_micros || 0).toFixed(1)} µs`);
        setText('db-stat-latency-range', `Min: ${dt.min_query_time_micros || 0} µs | Max: ${dt.max_query_time_micros || 0} µs`);
        setText('db-stat-rw-ratio', `${(dt.read_queries || 0).toLocaleString()} / ${(dt.write_queries || 0).toLocaleString()}`);
        setText('db-stat-size', formatBytes(dt.db_size_bytes || 0));
        setText('db-stat-total-records', `Records: ${(dt.total_records || 0).toLocaleString()}`);
        setText('db-stat-byte-growth', `+${formatBytes(dt.byte_growth_per_day || 0)}/d`);
        setText('db-stat-record-growth', `+${(dt.record_growth_per_day || 0).toFixed(1)} rec/d`);
      }
    } catch (e) {
      console.warn('Telemetry fetch error:', e);
    }
  }

  async fetchCaptures() {
    try {
      const [sumRes, packRes] = await Promise.all([
        fetch('/api/telemetry/capture_summary'),
        fetch('/api/telemetry/captures?limit=50')
      ]);

      const setText = (id, val) => {
        const el = document.getElementById(id);
        if (el) el.textContent = val;
      };

      if (sumRes.ok) {
        const sum = await sumRes.json();
        setText('cap-stat-samples', sum.total_samples || 0);
        setText('cap-stat-tx-rx', `TX: ${sum.tx_count || 0} | RX: ${sum.rx_count || 0}`);
        const avgPacket = (sum.avg_bytes_per_packet || sum.avg_comp_bytes || 0).toFixed(1);
        const avgRaw = (sum.avg_raw_bytes || 0).toFixed(1);
        const avgComp = (sum.avg_comp_bytes || 0).toFixed(1);
        setText('cap-stat-avg-packet', `${avgPacket} B`);
        setText('cap-stat-avg-packet-sub', `Raw: ${avgRaw} B | Comp: ${avgComp} B`);
        const avgUser = (sum.avg_bytes_per_packet_per_user || 0).toFixed(1);
        const users = sum.unique_users_count || 1;
        setText('cap-stat-avg-user-packet', `${avgUser} B`);
        setText('cap-stat-avg-user-sub', `Active Users: ${users}`);
        setText('cap-stat-raw', `${(sum.total_raw_bytes || 0).toLocaleString()} B`);
        setText('cap-stat-avg-raw', `Avg: ${(sum.avg_raw_bytes || 0).toFixed(1)} B`);
        setText('cap-stat-comp', `${(sum.total_comp_bytes || 0).toLocaleString()} B`);
        const saved = (sum.total_raw_bytes || 0) - (sum.total_comp_bytes || 0);
        setText('cap-stat-saved', `Saved: ${saved.toLocaleString()} B`);
        setText('cap-stat-savings', `+${(sum.net_savings_percent || 0).toFixed(2)}%`);
        setText('cap-stat-avg-time', `Avg Time: ${(sum.avg_duration_us || 0).toFixed(1)} µs`);
      }

      if (packRes.ok) {
        const data = await packRes.json();
        const tbody = document.getElementById('captures-tbody');
        tbody.innerHTML = '';

        (data.rows || []).forEach(r => {
          const tr = document.createElement('tr');
          tr.innerHTML = `
            <td>#${r.seq}</td>
            <td><strong>${r.direction}</strong></td>
            <td>${r.category}</td>
            <td><code>${r.opcode}</code></td>
            <td><code>${r.flags}</code></td>
            <td>${r.raw_bytes} B</td>
            <td>${r.compressed_bytes} B</td>
            <td class="${r.savings_percent > 0 ? 'text-success' : ''}">+${r.savings_percent.toFixed(1)}%</td>
            <td>${r.algorithm}</td>
            <td>${r.duration_us}</td>
          `;
          tbody.appendChild(tr);
        });
      }
    } catch (e) {
      console.warn('Captures fetch error:', e);
    }
  }

  async callSupervisorAction(action) {
    try {
      const res = await fetch(`/api/supervisor/${action}`, { method: 'POST' });
      if (res.ok) {
        this.fetchOverview();
        this.fetchServices();
      } else {
        alert(`Action '${action}' failed: ` + await res.text());
      }
    } catch (e) {
      alert(`Action '${action}' error: ` + e);
    }
  }

  async startCrawler(steps, delay) {
    try {
      const res = await fetch(`/api/supervisor/crawler?steps=${steps}&delay=${delay}`, { method: 'POST' });
      if (res.ok) {
        this.switchTab('logs');
      }
    } catch (e) {
      alert('Error starting crawler: ' + e);
    }
  }

  async runTuning(subcmd, extraArgs) {
    try {
      const res = await fetch(`/api/supervisor/tuning?command=${subcmd}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ args: extraArgs }),
      });
      if (res.ok) {
        this.switchTab('tuning');
      }
    } catch (e) {
      alert('Error running tuning: ' + e);
    }
  }

  // --- LOG BUFFERING & WEBSOCKET ---

  async fetchHistoricalLogs() {
    try {
      const res = await fetch('/api/logs?limit=5000');
      if (!res.ok) return;
      const history = await res.json();
      
      history.forEach(entry => {
        if (!this.logIdSet.has(entry.id)) {
          this.logIdSet.add(entry.id);
          this.logs.push(entry);
        }
      });

      this.logs.sort((a, b) => a.id - b.id);
      if (this.logs.length > 5000) {
        this.logs = this.logs.slice(this.logs.length - 5000);
      }

      this.renderLogs();
    } catch (e) {
      console.warn('Fetch historical logs error:', e);
    }
  }

  connectLogsWebSocket() {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/ws/logs`;
    this.logsWs = new WebSocket(url);

    this.logsWs.onmessage = (event) => {
      try {
        const entry = JSON.parse(event.data);
        if (!this.logIdSet.has(entry.id)) {
          this.logIdSet.add(entry.id);
          this.logs.push(entry);
          if (this.logs.length > 5000) {
            const evicted = this.logs.shift();
            this.logIdSet.delete(evicted.id);
          }

          if (this.matchesLogFilter(entry)) {
            this.appendLogEntry(entry);
          }
          this.updateLogCountBadge();
        }
      } catch (e) {
        console.warn('Log parse error:', e);
      }
    };

    this.logsWs.onclose = () => {
      setTimeout(() => this.connectLogsWebSocket(), 3000);
    };
  }

  matchesLogFilter(entry) {
    const lvlEl = document.getElementById('log-level-filter');
    const srcEl = document.getElementById('log-source-filter');
    const searchEl = document.getElementById('log-search-input');

    const lvlFilter = lvlEl ? lvlEl.value : 'ALL';
    const srcFilter = srcEl ? srcEl.value : 'ALL';
    const search = searchEl ? searchEl.value.toLowerCase() : '';

    if (lvlFilter !== 'ALL' && !entry.level.toUpperCase().includes(lvlFilter)) return false;
    if (srcFilter !== 'ALL' && !entry.source.toLowerCase().includes(srcFilter.toLowerCase())) return false;
    if (search && !entry.message.toLowerCase().includes(search)) return false;
    return true;
  }

  renderLogs() {
    const container = document.getElementById('log-entries') || document.getElementById('log-console');
    if (!container) return;
    container.innerHTML = '';
    
    this.logs.forEach(entry => {
      if (this.matchesLogFilter(entry)) {
        this.appendLogEntry(entry);
      }
    });

    this.updateLogCountBadge();
  }

  updateLogCountBadge() {
    const total = this.logs.length;
    const badge = document.getElementById('log-count-badge');
    if (badge) badge.textContent = total;
    const ind = document.getElementById('log-count-indicator');
    if (ind) ind.textContent = `${total} buffered logs`;
  }

  appendLogEntry(entry) {
    const container = document.getElementById('log-entries') || document.getElementById('log-console');
    const scrollParent = document.getElementById('log-console') || container;
    if (!container) return;

    const div = document.createElement('div');
    div.className = 'log-entry';
    div.innerHTML = `
      <span class="log-ts">${entry.timestamp ? entry.timestamp.slice(11, 23) : ''}</span>
      <span class="log-lvl log-lvl-${(entry.level || 'info').toLowerCase()}">[${entry.level || 'INFO'}]</span>
      <span class="log-src">&lt;${entry.source || 'sys'}&gt;</span>
      <span class="log-msg">${escapeHtml(entry.message || '')}</span>
    `;
    container.appendChild(div);

    if (this.autoScrollLogs && scrollParent) {
      scrollParent.scrollTop = scrollParent.scrollHeight;
    }
  }

  // --- TERMINAL WEBSOCKET & 80x25 CANVAS ---

  connectTerminalWebSocket() {
    if (this.termWs) {
      this.termWs.close();
    }
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${proto}//${location.host}/ws/terminal`;
    this.termWs = new WebSocket(url);

    const termGrid = document.getElementById('terminal-canvas-grid');
    const statusDot = document.getElementById('term-status-dot');
    const nodeBadge = document.getElementById('term-node-badge');

    this.termWs.onopen = () => {
      statusDot.className = 'term-status-dot active';
      termGrid.innerHTML = '<div class="term-line">Connected! Handshaking with virtual BBS session...</div>';
    };

    this.termWs.onmessage = (event) => {
      try {
        const msg = JSON.parse(event.data);
        if (msg.type === 'Connected') {
          nodeBadge.textContent = `NODE: ${msg.data.node_id.slice(0, 8)}...`;
        } else if (msg.type === 'ScreenUpdate') {
          this.renderTerminalScreen(msg.data.lines);
        }
      } catch (e) {
        console.warn('Terminal WS message error:', e);
      }
    };

    this.termWs.onclose = () => {
      statusDot.className = 'term-status-dot';
      termGrid.innerHTML = '<div class="term-line" style="color:#ff3333;">[SESSION DISCONNECTED] Click RECONNECT above to start new session.</div>';
    };
  }

  renderTerminalScreen(lines) {
    const grid = document.getElementById('terminal-canvas-grid');
    grid.innerHTML = '';

    (lines || []).forEach(lineHtml => {
      const rowDiv = document.createElement('div');
      rowDiv.className = 'term-line';
      rowDiv.innerHTML = lineHtml || '&nbsp;';
      grid.appendChild(rowDiv);
    });
  }

  sendTerminalKey(key) {
    if (this.termWs && this.termWs.readyState === WebSocket.OPEN) {
      this.termWs.send(JSON.stringify({ type: 'key', key: key }));
    }
  }

  // --- DATABASE MANAGER ---

  async fetchDatabase() {
    try {
      const res = await fetch('/api/database/summary');
      if (!res.ok) return;
      const data = await res.json();

      const pathEl = document.getElementById('db-file-path');
      if (pathEl) pathEl.textContent = data.path;
      const sizeEl = document.getElementById('db-file-size');
      if (sizeEl) sizeEl.textContent = `${(data.size_bytes || 0).toLocaleString()} bytes (${formatBytes(data.size_bytes || 0)})`;
      const totalTblEl = document.getElementById('db-total-tables');
      if (totalTblEl) totalTblEl.textContent = data.total_tables;
      const totalRecEl = document.getElementById('db-total-records');
      if (totalRecEl) totalRecEl.textContent = (data.total_records || 0).toLocaleString();

      const container = document.getElementById('db-tables-container');
      if (!container) return;
      container.innerHTML = '';

      if (!data.tables || data.tables.length === 0) {
        container.innerHTML = '<div class="empty-state" style="padding: 10px; font-size: 11px;">No tables found in database</div>';
        return;
      }

      data.tables.forEach(tbl => {
        const btn = document.createElement('button');
        btn.className = `retro-btn table-item-btn ${this.currentSelectedTable === tbl.namespace ? 'btn-active' : ''}`;
        btn.style.textAlign = 'left';
        btn.style.display = 'flex';
        btn.style.justifyContent = 'space-between';
        btn.style.alignItems = 'center';
        btn.style.padding = '6px 10px';
        btn.innerHTML = `
          <span><strong>${escapeHtml(tbl.namespace)}</strong></span>
          <span class="badge" style="font-size: 10px;">${(tbl.count || 0).toLocaleString()} recs | ${formatBytes(tbl.size_bytes || 0)}</span>
        `;
        btn.addEventListener('click', () => this.selectTable(tbl.namespace));
        container.appendChild(btn);
      });

      if (this.currentSelectedTable) {
        this.selectTable(this.currentSelectedTable, false);
      }
    } catch (e) {
      console.warn('Failed to fetch database summary:', e);
    }
  }

  async selectTable(namespace, refreshTables = true) {
    this.currentSelectedTable = namespace;
    const titleEl = document.getElementById('db-selected-table-title');
    if (titleEl) titleEl.textContent = `══ TABLE: ${namespace.toUpperCase()} ══`;
    const clearBtn = document.getElementById('btn-clear-table');
    if (clearBtn) clearBtn.style.display = 'inline-block';
    const actionsBar = document.getElementById('db-actions-bar');
    if (actionsBar) actionsBar.style.display = 'flex';

    // Highlight active in tables list
    document.querySelectorAll('.table-item-btn').forEach(btn => {
      if (btn.textContent.includes(namespace)) {
        btn.classList.add('btn-active');
      } else {
        btn.classList.remove('btn-active');
      }
    });

    try {
      const res = await fetch(`/api/database/table/${encodeURIComponent(namespace)}`);
      if (!res.ok) return;
      const records = await res.json();

      const tbody = document.getElementById('db-records-tbody');
      if (!tbody) return;
      tbody.innerHTML = '';

      if (!records || records.length === 0) {
        tbody.innerHTML = '<tr><td colspan="3" class="empty-state">Table is empty</td></tr>';
        return;
      }

      records.forEach(rec => {
        const tr = document.createElement('tr');
        tr.innerHTML = `
          <td style="font-family: monospace; font-weight: bold;">${escapeHtml(rec.key)}</td>
          <td style="font-family: monospace; word-break: break-all;">${escapeHtml(rec.value)}</td>
          <td style="text-align: center;">
            <button class="btn-small btn-danger delete-key-btn" data-key="${escapeHtml(rec.key)}">🗑 DEL</button>
          </td>
        `;
        tbody.appendChild(tr);
      });

      tbody.querySelectorAll('.delete-key-btn').forEach(btn => {
        btn.addEventListener('click', () => this.deleteKey(namespace, btn.dataset.key));
      });
    } catch (e) {
      console.warn('Failed to load table records:', e);
    }
  }

  async saveCurrentKey() {
    if (!this.currentSelectedTable) return;
    const keyInput = document.getElementById('db-new-key');
    const valInput = document.getElementById('db-new-val');
    const key = keyInput.value.trim();
    const val = valInput.value.trim();

    if (!key) {
      alert('Please specify a key.');
      return;
    }

    try {
      const res = await fetch(`/api/database/table/${encodeURIComponent(this.currentSelectedTable)}/key/${encodeURIComponent(key)}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ value: val }),
      });
      if (res.ok) {
        keyInput.value = '';
        valInput.value = '';
        this.fetchDatabase();
      } else {
        alert('Failed to save key.');
      }
    } catch (e) {
      alert('Error saving key: ' + e);
    }
  }

  async deleteKey(namespace, key) {
    if (!confirm(`Delete key "${key}" from table "${namespace}"?`)) return;
    try {
      const res = await fetch(`/api/database/table/${encodeURIComponent(namespace)}/key/${encodeURIComponent(key)}`, {
        method: 'DELETE',
      });
      if (res.ok) {
        this.fetchDatabase();
      }
    } catch (e) {
      console.warn('Failed to delete key:', e);
    }
  }

  async clearCurrentTable() {
    if (!this.currentSelectedTable) return;
    if (!confirm(`Clear all records in table "${this.currentSelectedTable}"? This action cannot be undone.`)) return;
    try {
      const res = await fetch(`/api/database/table/${encodeURIComponent(this.currentSelectedTable)}`, {
        method: 'DELETE',
      });
      if (res.ok) {
        this.fetchDatabase();
      }
    } catch (e) {
      console.warn('Failed to clear table:', e);
    }
  }
}

function escapeHtml(text) {
  const map = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#039;' };
  return text.replace(/[&<>"']/g, m => map[m]);
}

function formatBytes(bytes) {
  if (!bytes || bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return (bytes / Math.pow(k, i)).toFixed(i === 0 ? 0 : 1) + ' ' + sizes[i];
}

window.app = new HeimdallApp();
