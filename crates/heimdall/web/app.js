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
    // Nav Tabs
    document.querySelectorAll('.nav-tab').forEach(tabBtn => {
      tabBtn.addEventListener('click', () => {
        this.switchTab(tabBtn.dataset.tab);
      });
    });

    // Theme & Scanlines
    document.getElementById('theme-selector').addEventListener('change', (e) => {
      this.setTheme(e.target.value);
    });

    document.getElementById('toggle-scanlines').addEventListener('click', () => {
      const isOverlay = document.body.classList.toggle('crt-overlay');
      const btn = document.getElementById('toggle-scanlines');
      btn.textContent = `CRT FX: ${isOverlay ? 'ON' : 'OFF'}`;
      btn.setAttribute('aria-pressed', isOverlay);
    });

    // Supervisor Quick Actions
    document.getElementById('btn-start-bbs').addEventListener('click', () => this.callSupervisorAction('start_bbs'));
    document.getElementById('btn-stop-bbs').addEventListener('click', () => this.callSupervisorAction('stop_bbs'));
    document.getElementById('btn-restart-bbs').addEventListener('click', () => this.callSupervisorAction('restart_bbs'));
    document.getElementById('btn-quick-crawler').addEventListener('click', () => this.startCrawler(100, 50));
    document.getElementById('btn-quick-benchmark').addEventListener('click', () => this.runTuning('analyze', []));

    // Logs Controls
    document.getElementById('log-level-filter').addEventListener('change', () => this.renderLogs());
    document.getElementById('log-source-filter').addEventListener('change', () => this.renderLogs());
    document.getElementById('log-search-input').addEventListener('input', () => this.renderLogs());
    document.getElementById('btn-refresh-logs').addEventListener('click', () => this.fetchHistoricalLogs());
    document.getElementById('btn-clear-logs').addEventListener('click', () => {
      this.logs = [];
      this.logIdSet.clear();
      this.renderLogs();
    });

    // Database Controls
    document.getElementById('btn-refresh-db').addEventListener('click', () => this.fetchDatabase());
    document.getElementById('btn-clear-table').addEventListener('click', () => this.clearCurrentTable());
    document.getElementById('btn-save-key').addEventListener('click', () => this.saveCurrentKey());

    // DB Backup / Restore / Reset
    document.getElementById('btn-backup-db').addEventListener('click', () => {
      window.location.href = '/api/database/backup';
    });

    const restoreInput = document.getElementById('db-restore-file-input');
    document.getElementById('btn-restore-db').addEventListener('click', () => {
      restoreInput.click();
    });
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

    const nukeModal = document.getElementById('modal-reset-db');
    const nukeInput = document.getElementById('input-nuke-confirm');
    const nukeConfirmBtn = document.getElementById('btn-confirm-nuke');
    const nukeCancelBtn = document.getElementById('btn-cancel-nuke');

    document.getElementById('btn-reset-db').addEventListener('click', () => {
      if (!confirm('RESET DATABASE (Step 1 of 2): Are you sure you want to reset the database? All tables will be wiped.')) {
        return;
      }
      nukeInput.value = '';
      nukeConfirmBtn.disabled = true;
      nukeModal.style.display = 'flex';
      nukeInput.focus();
    });

    nukeInput.addEventListener('input', () => {
      nukeConfirmBtn.disabled = nukeInput.value.trim().toUpperCase() !== 'NUKE IT';
    });

    nukeCancelBtn.addEventListener('click', () => {
      nukeModal.style.display = 'none';
      nukeInput.value = '';
    });

    nukeConfirmBtn.addEventListener('click', async () => {
      try {
        const res = await fetch('/api/database/reset', { method: 'POST' });
        if (res.ok) {
          alert('Database has been completely reset and vacuumed.');
          nukeModal.style.display = 'none';
          this.fetchDatabase();
          this.fetchOverview();
        } else {
          alert('Failed to reset database: ' + await res.text());
        }
      } catch (err) {
        alert('Error resetting database: ' + err);
      }
    });

    // Web Terminal Controls & Keyboard capture
    document.getElementById('btn-term-reconnect').addEventListener('click', () => this.connectTerminalWebSocket());
    document.getElementById('btn-term-reset').addEventListener('click', () => {
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

    const termScreen = document.getElementById('terminal-screen');
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
    document.getElementById('btn-save-app-file').addEventListener('click', () => this.saveCurrentAppFile());

    // Config Forms
    document.getElementById('config-form').addEventListener('submit', (e) => {
      e.preventDefault();
      this.saveConfigForm();
    });

    document.getElementById('btn-reload-config').addEventListener('click', () => this.fetchConfig());
    document.getElementById('btn-save-raw-toml').addEventListener('click', () => this.saveRawToml());

    // Tuning Console Actions
    document.getElementById('btn-run-analyze').addEventListener('click', () => this.runTuning('analyze', []));
    document.getElementById('btn-run-sweep').addEventListener('click', () => this.runTuning('sweep', []));
    document.getElementById('btn-run-train').addEventListener('click', () => {
      const tokens = document.getElementById('tuning-tokens-input').value || '254';
      this.runTuning('train', ['--tokens', tokens]);
    });
    document.getElementById('btn-run-crawler-custom').addEventListener('click', () => {
      const steps = parseInt(document.getElementById('crawler-steps-input').value, 10) || 100;
      this.startCrawler(steps, 50);
    });

    document.getElementById('btn-refresh-captures').addEventListener('click', () => this.fetchCaptures());
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

      if (bbs && bbs.state === 'running') {
        dot.className = 'indicator-dot active';
        text.textContent = `ONLINE (PID: ${bbs.pid || 'N/A'})`;
      } else {
        dot.className = 'indicator-dot stopped';
        text.textContent = bbs ? bbs.state.toUpperCase() : 'OFFLINE';
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

      document.getElementById('stat-active-sessions').textContent = s.active_sessions || '0';
      document.getElementById('stat-unique-users').textContent = `24h Users: ${s.unique_users_24h || 0}`;
      document.getElementById('stat-duty-cycle').textContent = `${(s.duty_cycle_percent || 0).toFixed(2)}%`;
      document.getElementById('duty-cycle-text').textContent = `${(s.duty_cycle_percent || 0).toFixed(2)}% / 1.0%`;
      document.getElementById('stat-packets-tx-rx').textContent = `${s.total_packets_sent || 0} / ${s.total_packets_received || 0}`;
      document.getElementById('stat-ppm').textContent = `PPM: ${(s.send_ppm_1h || 0).toFixed(1)} TX / ${(s.recv_ppm_1h || 0).toFixed(1)} RX`;
      document.getElementById('stat-compression-savings').textContent = `+${(s.compression_savings_percent || 0).toFixed(1)}%`;
      document.getElementById('stat-raw-comp-bytes').textContent = `Raw: ${s.total_raw_bytes_sent || 0} B | Comp: ${s.total_compressed_bytes_sent || 0} B`;

      // Surface DB numbers to Overview
      if (s.database_summary) {
        const dbSum = s.database_summary;
        const dbTel = s.database || dbSum.telemetry || {};
        const dbSizeEl = document.getElementById('stat-db-size');
        if (dbSizeEl) dbSizeEl.textContent = formatBytes(dbSum.size_bytes || 0);
        const dbSubEl = document.getElementById('stat-db-sub');
        if (dbSubEl) dbSubEl.textContent = `Records: ${(dbSum.total_records || 0).toLocaleString()} | Growth: ${formatBytes(dbTel.byte_growth_per_day || 0)}/d`;
      }

      // Populate Database Telemetry & Stats grid
      if (s.database) {
        const dt = s.database;
        const qphEl = document.getElementById('db-stat-qph');
        if (qphEl) qphEl.textContent = (dt.queries_per_hour || 0).toFixed(1);
        const totalQEl = document.getElementById('db-stat-total-queries');
        if (totalQEl) totalQEl.textContent = `Total: ${(dt.total_queries || 0).toLocaleString()} queries`;
        const avgTimeEl = document.getElementById('db-stat-avg-time');
        if (avgTimeEl) avgTimeEl.textContent = `${(dt.avg_query_time_micros || 0).toFixed(1)} µs`;
        const latRangeEl = document.getElementById('db-stat-latency-range');
        if (latRangeEl) latRangeEl.textContent = `Min: ${dt.min_query_time_micros || 0} µs | Max: ${dt.max_query_time_micros || 0} µs`;
        const rwRatioEl = document.getElementById('db-stat-rw-ratio');
        if (rwRatioEl) rwRatioEl.textContent = `${(dt.read_queries || 0).toLocaleString()} / ${(dt.write_queries || 0).toLocaleString()}`;
        const dbSizeEl = document.getElementById('db-stat-size');
        if (dbSizeEl) dbSizeEl.textContent = formatBytes(dt.db_size_bytes || 0);
        const totalRecEl = document.getElementById('db-stat-total-records');
        if (totalRecEl) totalRecEl.textContent = `Records: ${(dt.total_records || 0).toLocaleString()}`;
        const byteGrowthEl = document.getElementById('db-stat-byte-growth');
        if (byteGrowthEl) byteGrowthEl.textContent = `+${formatBytes(dt.byte_growth_per_day || 0)}/d`;
        const recGrowthEl = document.getElementById('db-stat-record-growth');
        if (recGrowthEl) recGrowthEl.textContent = `+${(dt.record_growth_per_day || 0).toFixed(1)} rec/d`;
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

      if (sumRes.ok) {
        const sum = await sumRes.json();
        document.getElementById('cap-stat-samples').textContent = sum.total_samples || 0;
        document.getElementById('cap-stat-tx-rx').textContent = `TX: ${sum.tx_count || 0} | RX: ${sum.rx_count || 0}`;
        const avgPacket = (sum.avg_bytes_per_packet || sum.avg_comp_bytes || 0).toFixed(1);
        const avgRaw = (sum.avg_raw_bytes || 0).toFixed(1);
        const avgComp = (sum.avg_comp_bytes || 0).toFixed(1);
        document.getElementById('cap-stat-avg-packet').textContent = `${avgPacket} B`;
        document.getElementById('cap-stat-avg-packet-sub').textContent = `Raw: ${avgRaw} B | Comp: ${avgComp} B`;
        const avgUser = (sum.avg_bytes_per_packet_per_user || 0).toFixed(1);
        const users = sum.unique_users_count || 1;
        document.getElementById('cap-stat-avg-user-packet').textContent = `${avgUser} B`;
        document.getElementById('cap-stat-avg-user-sub').textContent = `Active Users: ${users}`;
        document.getElementById('cap-stat-raw').textContent = `${(sum.total_raw_bytes || 0).toLocaleString()} B`;
        document.getElementById('cap-stat-avg-raw').textContent = `Avg: ${(sum.avg_raw_bytes || 0).toFixed(1)} B`;
        document.getElementById('cap-stat-comp').textContent = `${(sum.total_comp_bytes || 0).toLocaleString()} B`;
        const saved = (sum.total_raw_bytes || 0) - (sum.total_comp_bytes || 0);
        document.getElementById('cap-stat-saved').textContent = `Saved: ${saved.toLocaleString()} B`;
        document.getElementById('cap-stat-savings').textContent = `+${(sum.net_savings_percent || 0).toFixed(2)}%`;
        document.getElementById('cap-stat-avg-time').textContent = `Avg Time: ${(sum.avg_duration_us || 0).toFixed(1)} µs`;
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
    const lvlFilter = document.getElementById('log-level-filter').value;
    const srcFilter = document.getElementById('log-source-filter').value;
    const search = document.getElementById('log-search-input').value.toLowerCase();

    if (lvlFilter !== 'ALL' && !entry.level.toUpperCase().includes(lvlFilter)) return false;
    if (srcFilter !== 'ALL' && !entry.source.toLowerCase().includes(srcFilter.toLowerCase())) return false;
    if (search && !entry.message.toLowerCase().includes(search)) return false;
    return true;
  }

  renderLogs() {
    const consoleEl = document.getElementById('log-console');
    consoleEl.innerHTML = '';
    
    this.logs.forEach(entry => {
      if (this.matchesLogFilter(entry)) {
        this.appendLogEntry(entry);
      }
    });

    this.updateLogCountBadge();
  }

  updateLogCountBadge() {
    const total = this.logs.length;
    document.getElementById('log-count-badge').textContent = total;
    document.getElementById('log-count-indicator').textContent = `${total} buffered logs`;
  }

  appendLogEntry(entry) {
    const consoleEl = document.getElementById('log-console');
    const div = document.createElement('div');
    div.className = 'log-entry';
    div.innerHTML = `
      <span class="log-ts">${entry.timestamp.slice(11, 23)}</span>
      <span class="log-lvl log-lvl-${entry.level.toLowerCase()}">[${entry.level}]</span>
      <span class="log-src">&lt;${entry.source}&gt;</span>
      <span class="log-msg">${escapeHtml(entry.message)}</span>
    `;
    consoleEl.appendChild(div);

    if (document.getElementById('log-autoscroll').checked) {
      consoleEl.scrollTop = consoleEl.scrollHeight;
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
