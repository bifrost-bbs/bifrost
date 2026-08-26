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
    this.networkNodes = [];

    // Authentication & Identity State
    this.token = localStorage.getItem('heimdall_token') || null;
    this.currentUser = null;
    this.impersonating = null;
    this.allPermissions = [];
    this.usersList = [];
    this.setupRequired = false;
    
    this.init();
  }

  async apiFetch(url, options = {}) {
    options.headers = options.headers || {};
    if (this.token) {
      options.headers['Authorization'] = `Bearer ${this.token}`;
    }
    try {
      const res = await fetch(url, options);
      if (res.status === 401 && !url.includes('/api/auth/')) {
        this.token = null;
        localStorage.removeItem('heimdall_token');
        this.checkAuthStatus();
      }
      return res;
    } catch (e) {
      throw e;
    }
  }

  async init() {
    this.bindEvents();
    this.initTheme();
    this.startClock();
    
    // Check Authentication & Setup status first
    await this.checkAuthStatus();

    // Initial data fetches
    this.fetchOverview();
    this.fetchServices();
    this.fetchApps();
    this.fetchCatalog();
    this.fetchNetworkRegistry();
    this.fetchRadio();
    this.fetchConfig();
    this.fetchTelemetry();
    this.fetchCaptures();
    this.fetchDatabase();
    this.fetchHistoricalLogs();
    if (this.hasPerm('heimdall.users')) {
      this.fetchUsers();
    }

    // Connect Log Stream WebSocket
    this.connectLogsWebSocket();

    // Continuous Realtime Polling Interval (every 3 seconds)
    setInterval(() => {
      this.fetchOverview();
      this.fetchServices();
      if (this.activeTab === 'network') {
        this.fetchNetworkRegistry();
      }
      if (this.activeTab === 'radio') {
        this.fetchRadio();
      }
      if (this.activeTab === 'telemetry') {
        this.fetchTelemetry();
        this.fetchCaptures();
      }
      if (this.activeTab === 'database') {
        this.fetchDatabase();
      }
      if (this.activeTab === 'users' && this.hasPerm('heimdall.users')) {
        this.fetchUsers();
      }
    }, 3000);
  }

  hasPerm(perm) {
    if (!this.currentUser) return true; // fallback if open
    if (this.currentUser.is_admin) return true;
    const perms = this.currentUser.permissions || [];
    return perms.includes('admin') || perms.includes('*') || perms.includes(perm);
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

    // Auth & Profile Controls
    on('btn-submit-auth', 'click', () => this.submitAuthForm());
    on('auth-password', 'keydown', (e) => {
      if (e.key === 'Enter') this.submitAuthForm();
    });
    on('auth-confirm-password', 'keydown', (e) => {
      if (e.key === 'Enter') this.submitAuthForm();
    });
    on('btn-logout', 'click', () => this.logout());
    on('btn-open-change-password', 'click', () => this.openChangePasswordModal());
    on('btn-confirm-change-pass', 'click', () => this.submitChangePassword());
    on('btn-cancel-change-pass', 'click', () => {
      const m = document.getElementById('modal-change-password');
      if (m) m.style.display = 'none';
    });

    // Impersonation Banner
    on('btn-stop-impersonate', 'click', () => this.stopImpersonating());

    // Users Tab Controls & Modals
    on('btn-refresh-users', 'click', () => this.fetchUsers());
    on('users-search-input', 'input', () => this.renderUsers());
    on('btn-open-create-user', 'click', () => this.openCreateUserModal());
    on('btn-confirm-create-user', 'click', () => this.submitCreateUser());
    on('btn-cancel-create-user', 'click', () => {
      const m = document.getElementById('modal-create-user');
      if (m) m.style.display = 'none';
    });
    on('btn-save-edit-perms', 'click', () => this.submitEditPermissions());
    on('btn-cancel-edit-perms', 'click', () => {
      const m = document.getElementById('modal-edit-permissions');
      if (m) m.style.display = 'none';
    });
    on('btn-confirm-reset-pass', 'click', () => this.submitResetPassword());
    on('btn-cancel-reset-pass', 'click', () => {
      const m = document.getElementById('modal-reset-password');
      if (m) m.style.display = 'none';
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

    // App Store Catalog Controls
    on('btn-refresh-catalog', 'click', () => this.fetchCatalog(true));
    on('store-category-filter', 'change', () => this.renderCatalog());
    on('store-search-input', 'input', () => this.renderCatalog());

    // Multi-BBS Network Controls
    on('btn-refresh-network', 'click', () => this.syncNetworkRegistry());
    on('network-search-input', 'input', (e) => this.filterNetworkNodes(e.target.value));

    // Radio Hardware Controls
    on('btn-refresh-radio', 'click', () => this.fetchRadio());
    on('radio-config-form', 'submit', (e) => {
      e.preventDefault();
      this.saveRadioConfig();
    });

    // Database Controls
    on('btn-refresh-db', 'click', () => this.fetchDatabase());
    on('btn-clear-table', 'click', () => this.clearCurrentTable());
    on('btn-save-key', 'click', () => this.saveCurrentKey());

    // DB Backup / Restore / Reset
    on('btn-backup-db', 'click', () => {
      window.location.href = '/api/database/backup' + (this.token ? `?token=${encodeURIComponent(this.token)}` : '');
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
          const res = await this.apiFetch('/api/database/restore', {
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
          const res = await this.apiFetch('/api/database/reset', { method: 'POST' });
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

  async checkAuthStatus() {
    try {
      const res = await this.apiFetch('/api/auth/status');
      if (!res.ok) return;
      const data = await res.json();
      
      this.setupRequired = !!data.setup_required;
      this.allPermissions = data.all_permissions || [];
      this.impersonating = data.impersonating || null;

      const authModal = document.getElementById('auth-modal');
      const userBadge = document.getElementById('current-user-badge');
      const impBanner = document.getElementById('impersonation-banner');

      if (this.setupRequired) {
        this.showAuthModal(true);
        if (userBadge) userBadge.textContent = 'Setup Required';
      } else if (!data.authenticated) {
        this.currentUser = null;
        this.showAuthModal(false);
        if (userBadge) userBadge.textContent = 'Not Logged In';
      } else {
        this.currentUser = data.user;
        if (authModal) authModal.style.display = 'none';

        if (userBadge && this.currentUser) {
          const role = this.currentUser.is_admin ? 'ADMIN' : 'USER';
          userBadge.textContent = `${this.currentUser.nickname} [${role}]`;
        }

        // Impersonation Banner
        if (impBanner) {
          if (this.impersonating) {
            impBanner.style.display = 'flex';
            document.getElementById('impersonating-user-name').textContent = this.currentUser.nickname;
            document.getElementById('impersonating-node-id').textContent = this.currentUser.id.slice(0, 16) + '...';
          } else {
            impBanner.style.display = 'none';
          }
        }

        // Update tab accessibility
        this.updateTabPermissions();
      }
    } catch (e) {
      console.warn('Auth status check failed:', e);
    }
  }

  showAuthModal(isSetup) {
    const modal = document.getElementById('auth-modal');
    const title = document.getElementById('auth-modal-title');
    const desc = document.getElementById('auth-modal-desc');
    const confirmGroup = document.getElementById('auth-confirm-group');
    const submitBtn = document.getElementById('btn-submit-auth');
    const errorMsg = document.getElementById('auth-error-msg');

    if (!modal) return;
    if (errorMsg) errorMsg.style.display = 'none';

    if (isSetup) {
      if (title) title.textContent = '══ FIRST TIME SETUP: CREATE ADMINISTRATOR ══';
      if (desc) desc.textContent = 'Welcome to Bifrost MeshBBS! Create your root administrative account to initialize the BBS node.';
      if (confirmGroup) confirmGroup.style.display = 'block';
      if (submitBtn) submitBtn.textContent = 'CREATE ROOT ADMIN & LOGIN';
    } else {
      if (title) title.textContent = '══ HEIMDALL AUTHENTICATION ══';
      if (desc) desc.textContent = 'Enter your BBS nickname and password to access Heimdall.';
      if (confirmGroup) confirmGroup.style.display = 'none';
      if (submitBtn) submitBtn.textContent = 'LOGIN TO HEIMDALL';
    }

    modal.style.display = 'flex';
    const userInp = document.getElementById('auth-username');
    if (userInp) userInp.focus();
  }

  async submitAuthForm() {
    const username = (document.getElementById('auth-username').value || '').trim();
    const password = document.getElementById('auth-password').value || '';
    const confirmPassword = document.getElementById('auth-confirm-password').value || '';
    const errorMsg = document.getElementById('auth-error-msg');

    const showError = (msg) => {
      if (errorMsg) {
        errorMsg.textContent = msg;
        errorMsg.style.display = 'block';
      }
    };

    if (!username || !password) {
      showError('Nickname and password are required.');
      return;
    }

    if (this.setupRequired) {
      if (password !== confirmPassword) {
        showError('Passwords do not match.');
        return;
      }
      try {
        const res = await fetch('/api/auth/setup', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username, password }),
        });
        if (!res.ok) {
          showError(await res.text());
          return;
        }
        const data = await res.json();
        this.token = data.token;
        localStorage.setItem('heimdall_token', this.token);
        document.getElementById('auth-password').value = '';
        document.getElementById('auth-confirm-password').value = '';
        await this.checkAuthStatus();
        this.fetchOverview();
        if (this.hasPerm('heimdall.users')) this.fetchUsers();
      } catch (e) {
        showError('Setup failed: ' + e);
      }
    } else {
      try {
        const res = await fetch('/api/auth/login', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ username, password }),
        });
        if (!res.ok) {
          showError('Invalid nickname or password.');
          return;
        }
        const data = await res.json();
        this.token = data.token;
        localStorage.setItem('heimdall_token', this.token);
        document.getElementById('auth-password').value = '';
        await this.checkAuthStatus();
        this.fetchOverview();
        if (this.hasPerm('heimdall.users')) this.fetchUsers();
      } catch (e) {
        showError('Login error: ' + e);
      }
    }
  }

  async logout() {
    try {
      await this.apiFetch('/api/auth/logout', { method: 'POST' });
    } catch (e) {}
    this.token = null;
    this.currentUser = null;
    this.impersonating = null;
    localStorage.removeItem('heimdall_token');
    if (this.termWs) this.termWs.close();
    await this.checkAuthStatus();
  }

  openChangePasswordModal() {
    const modal = document.getElementById('modal-change-password');
    const err = document.getElementById('change-pass-error');
    if (err) err.style.display = 'none';
    document.getElementById('change-old-pass').value = '';
    document.getElementById('change-new-pass').value = '';
    document.getElementById('change-confirm-new-pass').value = '';
    if (modal) modal.style.display = 'flex';
  }

  async submitChangePassword() {
    const oldPass = document.getElementById('change-old-pass').value;
    const newPass = document.getElementById('change-new-pass').value;
    const confirmPass = document.getElementById('change-confirm-new-pass').value;
    const err = document.getElementById('change-pass-error');

    const showErr = (m) => {
      if (err) { err.textContent = m; err.style.display = 'block'; }
    };

    if (!oldPass || !newPass) {
      showErr('All password fields are required.');
      return;
    }
    if (newPass !== confirmPass) {
      showErr('New passwords do not match.');
      return;
    }

    try {
      const res = await this.apiFetch('/api/auth/change_password', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ old_password: oldPass, new_password: newPass }),
      });
      if (res.ok) {
        alert('Password updated successfully!');
        const modal = document.getElementById('modal-change-password');
        if (modal) modal.style.display = 'none';
      } else {
        showErr(await res.text());
      }
    } catch (e) {
      showErr('Error updating password: ' + e);
    }
  }

  async stopImpersonating() {
    try {
      const res = await this.apiFetch('/api/auth/stop_impersonating', { method: 'POST' });
      if (res.ok) {
        const data = await res.json();
        this.token = data.token;
        localStorage.setItem('heimdall_token', this.token);
        if (this.termWs) {
          this.termWs.close();
          this.connectTerminalWebSocket();
        }
        await this.checkAuthStatus();
        this.fetchOverview();
        if (this.hasPerm('heimdall.users')) this.fetchUsers();
      } else {
        alert('Failed to stop impersonating: ' + await res.text());
      }
    } catch (e) {
      alert('Error stopping impersonation: ' + e);
    }
  }

  updateTabPermissions() {
    const permMap = {
      'overview': 'heimdall.overview',
      'terminal': 'heimdall.terminal',
      'logs': 'heimdall.logs',
      'apps': 'heimdall.apps',
      'config': 'heimdall.config',
      'telemetry': 'heimdall.telemetry',
      'tuning': 'heimdall.tuning',
      'database': 'heimdall.database',
      'users': 'heimdall.users',
      'store': 'heimdall.apps',
    };

    document.querySelectorAll('.nav-tab').forEach(tabBtn => {
      const tabId = tabBtn.dataset.tab;
      const reqPerm = permMap[tabId];
      const allowed = !reqPerm || this.hasPerm(reqPerm);
      tabBtn.style.display = allowed ? '' : 'none';
    });
  }

  switchTab(tabId) {
    const permMap = {
      'overview': 'heimdall.overview',
      'terminal': 'heimdall.terminal',
      'logs': 'heimdall.logs',
      'apps': 'heimdall.apps',
      'config': 'heimdall.config',
      'telemetry': 'heimdall.telemetry',
      'tuning': 'heimdall.tuning',
      'database': 'heimdall.database',
      'users': 'heimdall.users',
      'store': 'heimdall.apps',
      'network': 'heimdall.overview',
      'radio': 'heimdall.config',
    };

    const reqPerm = permMap[tabId];
    if (reqPerm && !this.hasPerm(reqPerm)) {
      alert(`Access Denied: You do not have permission '${reqPerm}'`);
      return;
    }

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
    } else if (tabId === 'users') {
      this.fetchUsers();
    } else if (tabId === 'store') {
      this.fetchCatalog();
    } else if (tabId === 'network') {
      this.fetchNetworkRegistry();
    } else if (tabId === 'radio') {
      this.fetchRadio();
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
      const res = await this.apiFetch('/api/supervisor/status');
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
      const res = await this.apiFetch('/api/supervisor/status');
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
      const res = await this.apiFetch('/api/apps');
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
      const res = await this.apiFetch(`/api/apps/${appId}/files`);
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
      const res = await this.apiFetch(`/api/apps/${appId}/file_content?path=${encodeURIComponent(filePath)}`);
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
      const res = await this.apiFetch(`/api/apps/${this.currentSelectedApp}/file_content?path=${encodeURIComponent(this.currentEditingFile)}`, {
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

  // --- APP STORE & CATALOG METHODS ---

  async fetchCatalog(forceRefresh = false) {
    try {
      const res = await this.apiFetch(`/api/catalog?refresh=${forceRefresh}`);
      if (!res.ok) return;
      const data = await res.json();
      this.catalogData = data;
      const upd = document.getElementById('store-updated-at');
      if (upd && data.updated_at) {
        upd.textContent = `Catalog v${data.catalog_version} (Updated ${new Date(data.updated_at).toLocaleDateString()})`;
      }
      this.renderCatalog();
    } catch (e) {
      console.warn('Fetch catalog error:', e);
    }
  }

  renderCatalog() {
    if (!this.catalogData || !this.catalogData.apps) return;
    const grid = document.getElementById('store-grid');
    if (!grid) return;
    grid.innerHTML = '';

    const catFilter = document.getElementById('store-category-filter')?.value || 'ALL';
    const searchFilter = (document.getElementById('store-search-input')?.value || '').toLowerCase().trim();

    const filtered = this.catalogData.apps.filter(app => {
      if (catFilter !== 'ALL' && app.category !== catFilter) return false;
      if (searchFilter) {
        const text = `${app.name} ${app.id} ${app.description} ${app.author}`.toLowerCase();
        if (!text.includes(searchFilter)) return false;
      }
      return true;
    });

    if (filtered.length === 0) {
      grid.innerHTML = '<div style="grid-column: 1 / -1; padding: 30px; text-align: center; color: var(--text-muted);">No applications found matching your filter criteria.</div>';
      return;
    }

    filtered.forEach(app => {
      const card = document.createElement('div');
      card.className = 'retro-panel';
      card.style.padding = '14px';
      card.style.display = 'flex';
      card.style.flexDirection = 'column';
      card.style.justifyContent = 'space-between';
      card.style.position = 'relative';

      const icon = app.icon || '📦';
      let statusBadge = '';
      if (app.is_builtin) {
        statusBadge = '<span class="badge" style="background: #2a5a7a; color: #fff;">BUILT-IN</span>';
      } else if (app.installed) {
        if (app.update_available) {
          statusBadge = `<span class="badge" style="background: #e6a100; color: #000;">UPDATE: v${app.installed_version} → v${app.latest_version}</span>`;
        } else {
          statusBadge = `<span class="badge" style="background: var(--success); color: #000;">INSTALLED v${app.installed_version}</span>`;
        }
      } else {
        statusBadge = `<span class="badge" style="background: var(--border-color); color: var(--text-main);">v${app.latest_version}</span>`;
      }

      let actionsHtml = '';
      if (app.is_builtin) {
        actionsHtml = `<div style="display: flex; gap: 8px; align-items: center;"><span class="text-muted" style="font-size: 11px;">Core System App</span></div>`;
      } else if (!app.installed) {
        const releaseOptions = (app.releases || []).map(r => `<option value="${r.tag}">${r.tag} (${r.version})</option>`).join('');
        actionsHtml = `
          <div style="display: flex; gap: 6px; align-items: center; width: 100%;">
            <select class="retro-select store-tag-select" id="tag-select-${app.id}" style="flex: 1; font-size: 11px; padding: 4px;">
              ${releaseOptions || `<option value="${app.latest_tag}">${app.latest_tag}</option>`}
            </select>
            <button class="retro-btn btn-sm btn-success btn-install-app" data-app="${app.id}" style="padding: 4px 10px;">+ INSTALL</button>
          </div>
        `;
      } else {
        actionsHtml = `
          <div style="display: flex; gap: 6px; align-items: center; width: 100%; flex-wrap: wrap;">
            ${app.update_available ? `<button class="retro-btn btn-sm btn-warning btn-update-app" data-app="${app.id}" data-tag="${app.latest_tag}" style="flex: 1;">⬆ UPDATE (v${app.latest_version})</button>` : ''}
            <button class="retro-btn btn-sm ${app.enabled ? 'btn-danger' : 'btn-success'} btn-toggle-app" data-app="${app.id}" data-enabled="${!app.enabled}" style="flex: 1;">
              ${app.enabled ? 'DISABLE' : 'ENABLE'}
            </button>
            <button class="retro-btn btn-sm btn-danger btn-uninstall-app" data-app="${app.id}" title="Uninstall Application">🗑</button>
          </div>
        `;
      }

      card.innerHTML = `
        <div>
          <div style="display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 6px;">
            <div style="display: flex; align-items: center; gap: 8px;">
              <span style="font-size: 24px;">${icon}</span>
              <div>
                <strong style="font-size: 14px; color: var(--accent);">${app.name}</strong>
                <div style="font-size: 11px; color: var(--text-muted);">${app.id} · by ${app.author}</div>
              </div>
            </div>
            ${statusBadge}
          </div>
          <p style="font-size: 12px; margin: 8px 0; color: var(--text-main); line-height: 1.4;">${app.description}</p>
          <div style="margin-bottom: 12px; font-size: 11px;">
            <span class="badge" style="background: rgba(255,255,255,0.08);">${app.category.toUpperCase()}</span>
            <a href="${app.repository}" target="_blank" rel="noopener noreferrer" style="color: var(--accent); margin-left: 8px; text-decoration: none;">GitHub ↗</a>
          </div>
        </div>
        <div style="margin-top: auto; padding-top: 10px; border-top: 1px dashed var(--border-color);">
          ${actionsHtml}
        </div>
      `;

      grid.appendChild(card);
    });

    grid.querySelectorAll('.btn-install-app').forEach(btn => {
      btn.addEventListener('click', async () => {
        const appId = btn.dataset.app;
        const tagSelect = document.getElementById(`tag-select-${appId}`);
        const tag = tagSelect ? tagSelect.value : null;
        await this.installCatalogApp(appId, tag);
      });
    });

    grid.querySelectorAll('.btn-update-app').forEach(btn => {
      btn.addEventListener('click', async () => {
        const appId = btn.dataset.app;
        const tag = btn.dataset.tag;
        await this.installCatalogApp(appId, tag);
      });
    });

    grid.querySelectorAll('.btn-toggle-app').forEach(btn => {
      btn.addEventListener('click', async () => {
        const appId = btn.dataset.app;
        const enabled = btn.dataset.enabled === 'true';
        await this.toggleApp(appId, enabled);
      });
    });

    grid.querySelectorAll('.btn-uninstall-app').forEach(btn => {
      btn.addEventListener('click', async () => {
        const appId = btn.dataset.app;
        if (confirm(`Are you sure you want to uninstall '${appId}'? This will delete the app files.`)) {
          await this.uninstallCatalogApp(appId);
        }
      });
    });
  }

  async installCatalogApp(appId, tag) {
    try {
      const res = await this.apiFetch('/api/catalog/install', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ app_id: appId, tag: tag || null }),
      });
      if (res.ok) {
        alert(`Application '${appId}' successfully installed/updated and enabled!`);
        await this.fetchCatalog(true);
        await this.fetchApps();
        await this.fetchConfig();
      } else {
        alert(`Failed to install '${appId}': ` + await res.text());
      }
    } catch (e) {
      alert(`Error installing '${appId}': ` + e);
    }
  }

  async uninstallCatalogApp(appId) {
    try {
      const res = await this.apiFetch('/api/catalog/uninstall', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ app_id: appId }),
      });
      if (res.ok) {
        alert(`Application '${appId}' uninstalled.`);
        await this.fetchCatalog(true);
        await this.fetchApps();
        await this.fetchConfig();
      } else {
        alert(`Failed to uninstall '${appId}': ` + await res.text());
      }
    } catch (e) {
      alert(`Error uninstalling '${appId}': ` + e);
    }
  }

  async toggleApp(appId, enabled) {
    try {
      const res = await this.apiFetch(`/api/apps/${appId}/toggle`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled }),
      });
      if (res.ok) {
        await this.fetchCatalog(false);
        await this.fetchApps();
        await this.fetchConfig();
      } else {
        alert(`Failed to toggle '${appId}': ` + await res.text());
      }
    } catch (e) {
      alert(`Error toggling '${appId}': ` + e);
    }
  }

  async fetchConfig() {
    try {
      const res = await this.apiFetch('/api/config');
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
    const currentEnabled = this.currentConfig?.apps?.enabled || ["messages", "profile", "admin"];
    const enabledJson = JSON.stringify(currentEnabled, null, 4);

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
enabled = ${enabledJson}

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
      const res = await this.apiFetch('/api/config', {
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
      const res = await this.apiFetch('/api/telemetry/summary');
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
        this.apiFetch('/api/telemetry/capture_summary'),
        this.apiFetch('/api/telemetry/captures?limit=50')
      ]);

      const setText = (id, val) => {
        const el = document.getElementById(id);
        if (el) el.textContent = val;
      };

      if (sumRes && sumRes.ok) {
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

      if (packRes && packRes.ok) {
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
      const res = await this.apiFetch(`/api/supervisor/${action}`, { method: 'POST' });
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
      const res = await this.apiFetch(`/api/supervisor/crawler?steps=${steps}&delay=${delay}`, { method: 'POST' });
      if (res.ok) {
        this.switchTab('logs');
      }
    } catch (e) {
      alert('Error starting crawler: ' + e);
    }
  }

  async runTuning(subcmd, extraArgs) {
    try {
      const res = await this.apiFetch(`/api/supervisor/tuning?command=${subcmd}`, {
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
      const res = await this.apiFetch('/api/logs?limit=5000');
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
    const url = `${proto}//${location.host}/ws/logs` + (this.token ? `?token=${encodeURIComponent(this.token)}` : '');
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
    const url = `${proto}//${location.host}/ws/terminal` + (this.token ? `?token=${encodeURIComponent(this.token)}` : '');
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

  // --- USER MANAGEMENT ---

  async fetchUsers() {
    try {
      const res = await this.apiFetch('/api/users');
      if (!res.ok) return;
      this.usersList = await res.json();
      this.renderUsers();
    } catch (e) {
      console.warn('Fetch users error:', e);
    }
  }

  renderUsers() {
    const tbody = document.getElementById('users-tbody');
    const searchInp = document.getElementById('users-search-input');
    const countLabel = document.getElementById('users-count-label');
    if (!tbody) return;

    const filter = searchInp ? searchInp.value.trim().toLowerCase() : '';
    const filtered = this.usersList.filter(u => {
      if (!filter) return true;
      return u.nickname.toLowerCase().includes(filter) || u.id.toLowerCase().includes(filter);
    });

    if (countLabel) countLabel.textContent = `Total Users: ${this.usersList.length}`;
    tbody.innerHTML = '';

    if (filtered.length === 0) {
      tbody.innerHTML = '<tr><td colspan="5" class="empty-state">No users found</td></tr>';
      return;
    }

    filtered.forEach(u => {
      const tr = document.createElement('tr');
      const isSelf = this.currentUser && this.currentUser.id === u.id;
      
      // Permissions tags
      let permsBadges = '';
      if (u.is_admin) {
        permsBadges += '<span class="role-badge role-badge-admin">ADMIN</span>';
      } else {
        permsBadges += '<span class="role-badge role-badge-user">USER</span>';
      }
      (u.permissions || []).forEach(p => {
        if (p !== 'admin') {
          permsBadges += `<span class="role-badge role-badge-perm">${escapeHtml(p)}</span>`;
        }
      });

      tr.innerHTML = `
        <td><strong>${escapeHtml(u.nickname)}</strong> ${isSelf ? '<span style="color:var(--term-accent);font-size:10px;">(YOU)</span>' : ''}</td>
        <td><code style="font-size:11px;">${u.id.slice(0, 16)}...</code></td>
        <td>${permsBadges}</td>
        <td style="text-align: center;">${u.has_password ? '✅ SET' : '❌ NONE'}</td>
        <td style="text-align: center; white-space: nowrap;">
          <button class="retro-btn btn-sm impersonate-btn" data-id="${u.id}" ${isSelf ? 'disabled' : ''} title="Impersonate User">🎭 Impersonate</button>
          <button class="retro-btn btn-sm edit-perms-btn" data-id="${u.id}" title="Edit Permissions">🛡️ Perms</button>
          <button class="retro-btn btn-sm reset-pass-btn" data-id="${u.id}" data-nick="${escapeHtml(u.nickname)}" title="Reset Password">🔑 Pass</button>
          <button class="retro-btn btn-sm btn-danger delete-user-btn" data-id="${u.id}" data-nick="${escapeHtml(u.nickname)}" ${isSelf ? 'disabled' : ''} title="Delete User">🗑️</button>
        </td>
      `;
      tbody.appendChild(tr);
    });

    // Attach event listeners
    tbody.querySelectorAll('.impersonate-btn').forEach(btn => {
      btn.addEventListener('click', () => this.impersonateUser(btn.dataset.id));
    });
    tbody.querySelectorAll('.edit-perms-btn').forEach(btn => {
      btn.addEventListener('click', () => this.openEditPermissionsModal(btn.dataset.id));
    });
    tbody.querySelectorAll('.reset-pass-btn').forEach(btn => {
      btn.addEventListener('click', () => this.openResetPasswordModal(btn.dataset.id, btn.dataset.nick));
    });
    tbody.querySelectorAll('.delete-user-btn').forEach(btn => {
      btn.addEventListener('click', () => this.deleteUser(btn.dataset.id, btn.dataset.nick));
    });
  }

  openCreateUserModal() {
    const modal = document.getElementById('modal-create-user');
    const err = document.getElementById('create-user-error');
    if (err) err.style.display = 'none';
    document.getElementById('create-username').value = '';
    document.getElementById('create-password').value = '';

    const list = document.getElementById('create-user-perms-list');
    if (list) {
      list.innerHTML = '';
      this.allPermissions.forEach(p => {
        const label = document.createElement('label');
        label.className = 'perm-item-label';
        const isChecked = ['heimdall.login', 'heimdall.overview', 'heimdall.terminal', 'read', 'write'].includes(p.id);
        label.innerHTML = `
          <input type="checkbox" value="${p.id}" ${isChecked ? 'checked' : ''}>
          <span><strong>${escapeHtml(p.id)}</strong> (${escapeHtml(p.name)})</span>
        `;
        list.appendChild(label);
      });
    }

    if (modal) modal.style.display = 'flex';
  }

  async submitCreateUser() {
    const username = (document.getElementById('create-username').value || '').trim();
    const password = document.getElementById('create-password').value || '';
    const err = document.getElementById('create-user-error');

    const showErr = (m) => {
      if (err) { err.textContent = m; err.style.display = 'block'; }
    };

    if (!username || !password) {
      showErr('Nickname and password are required.');
      return;
    }

    const selectedPerms = [];
    document.querySelectorAll('#create-user-perms-list input[type="checkbox"]:checked').forEach(cb => {
      selectedPerms.push(cb.value);
    });

    try {
      const res = await this.apiFetch('/api/users', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          username,
          password,
          permissions: selectedPerms,
        }),
      });
      if (res.ok) {
        alert(`User '${username}' created successfully!`);
        const modal = document.getElementById('modal-create-user');
        if (modal) modal.style.display = 'none';
        this.fetchUsers();
      } else {
        showErr(await res.text());
      }
    } catch (e) {
      showErr('Error creating user: ' + e);
    }
  }

  openEditPermissionsModal(nodeId) {
    const user = this.usersList.find(u => u.id === nodeId);
    if (!user) return;
    this.editingUserNodeId = nodeId;

    document.getElementById('edit-perm-username').textContent = user.nickname;
    document.getElementById('edit-perm-node-id').textContent = nodeId;
    const err = document.getElementById('edit-perms-error');
    if (err) err.style.display = 'none';

    const list = document.getElementById('edit-perms-list');
    if (list) {
      list.innerHTML = '';
      this.allPermissions.forEach(p => {
        const isChecked = (user.permissions || []).includes(p.id) || (user.is_admin && p.id === 'admin');
        const label = document.createElement('label');
        label.className = 'perm-item-label';
        label.innerHTML = `
          <input type="checkbox" value="${p.id}" ${isChecked ? 'checked' : ''}>
          <span><strong>${escapeHtml(p.id)}</strong> (${escapeHtml(p.name)})</span>
        `;
        list.appendChild(label);
      });
    }

    const modal = document.getElementById('modal-edit-permissions');
    if (modal) modal.style.display = 'flex';
  }

  async submitEditPermissions() {
    if (!this.editingUserNodeId) return;
    const selectedPerms = [];
    document.querySelectorAll('#edit-perms-list input[type="checkbox"]:checked').forEach(cb => {
      selectedPerms.push(cb.value);
    });

    try {
      const res = await this.apiFetch(`/api/users/${this.editingUserNodeId}/permissions`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ permissions: selectedPerms }),
      });
      if (res.ok) {
        alert('Permissions updated successfully!');
        const modal = document.getElementById('modal-edit-permissions');
        if (modal) modal.style.display = 'none';
        this.fetchUsers();
      } else {
        alert('Failed to update permissions: ' + await res.text());
      }
    } catch (e) {
      alert('Error updating permissions: ' + e);
    }
  }

  openResetPasswordModal(nodeId, nickname) {
    this.resettingUserNodeId = nodeId;
    document.getElementById('reset-pass-username').textContent = nickname;
    document.getElementById('input-reset-new-pass').value = '';
    const err = document.getElementById('reset-pass-error');
    if (err) err.style.display = 'none';

    const modal = document.getElementById('modal-reset-password');
    if (modal) modal.style.display = 'flex';
  }

  async submitResetPassword() {
    if (!this.resettingUserNodeId) return;
    const newPass = document.getElementById('input-reset-new-pass').value;
    const err = document.getElementById('reset-pass-error');

    if (!newPass) {
      if (err) { err.textContent = 'Please enter a new password.'; err.style.display = 'block'; }
      return;
    }

    try {
      const res = await this.apiFetch(`/api/users/${this.resettingUserNodeId}/reset_password`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ new_password: newPass }),
      });
      if (res.ok) {
        alert('Password reset successfully!');
        const modal = document.getElementById('modal-reset-password');
        if (modal) modal.style.display = 'none';
        this.fetchUsers();
      } else {
        if (err) { err.textContent = await res.text(); err.style.display = 'block'; }
      }
    } catch (e) {
      if (err) { err.textContent = 'Error resetting password: ' + e; err.style.display = 'block'; }
    }
  }

  async impersonateUser(nodeId) {
    if (!confirm('Impersonate this user? You will adopt their identity and permissions. You can return to your admin session at any time.')) {
      return;
    }
    try {
      const res = await this.apiFetch(`/api/users/${nodeId}/impersonate`, { method: 'POST' });
      if (res.ok) {
        const data = await res.json();
        this.token = data.token;
        localStorage.setItem('heimdall_token', this.token);
        if (this.termWs) {
          this.termWs.close();
          this.connectTerminalWebSocket();
        }
        await this.checkAuthStatus();
        this.fetchOverview();
        if (this.hasPerm('heimdall.users')) this.fetchUsers();
      } else {
        alert('Impersonation failed: ' + await res.text());
      }
    } catch (e) {
      alert('Error impersonating user: ' + e);
    }
  }

  async deleteUser(nodeId, nickname) {
    if (!confirm(`Delete user "${nickname}" (${nodeId.slice(0, 16)}...)? This cannot be undone.`)) {
      return;
    }
    try {
      const res = await this.apiFetch(`/api/users/${nodeId}`, { method: 'DELETE' });
      if (res.ok) {
        this.fetchUsers();
      } else {
        alert('Failed to delete user: ' + await res.text());
      }
    } catch (e) {
      alert('Error deleting user: ' + e);
    }
  }

  // --- DATABASE MANAGER ---

  async fetchDatabase() {
    try {
      const res = await this.apiFetch('/api/database/summary');
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
      const res = await this.apiFetch(`/api/database/table/${encodeURIComponent(namespace)}`);
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
      const res = await this.apiFetch(`/api/database/table/${encodeURIComponent(this.currentSelectedTable)}/key/${encodeURIComponent(key)}`, {
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
      const res = await this.apiFetch(`/api/database/table/${encodeURIComponent(namespace)}/key/${encodeURIComponent(key)}`, {
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
      const res = await this.apiFetch(`/api/database/table/${encodeURIComponent(this.currentSelectedTable)}`, {
        method: 'DELETE',
      });
      if (res.ok) {
        this.fetchDatabase();
      }
    } catch (e) {
      console.warn('Failed to clear table:', e);
    }
  }

  // --- MULTI-BBS NETWORK REGISTRY METHODS ---

  async fetchNetworkRegistry() {
    try {
      const res = await this.apiFetch('/api/network');
      if (!res.ok) return;
      const data = await res.json();
      this.networkNodes = data.nodes || [];

      // Update Summary Cards
      const statusEl = document.getElementById('net-card-status');
      if (statusEl) {
        statusEl.textContent = data.network_enabled ? 'ACTIVE' : 'DISABLED';
        statusEl.style.color = data.network_enabled ? '#55ff55' : '#ff5555';
      }
      const totalEl = document.getElementById('net-card-total-nodes');
      if (totalEl) totalEl.textContent = (data.total_nodes || this.networkNodes.length).toString();

      const hopsEl = document.getElementById('net-card-max-hops');
      if (hopsEl) hopsEl.textContent = `${data.max_hops || 3} Hops`;

      const inboundEl = document.getElementById('net-card-inbound-status');
      if (inboundEl) {
        inboundEl.textContent = data.allow_inbound_relay ? 'ALLOWED' : 'BLOCKED';
        inboundEl.style.color = data.allow_inbound_relay ? '#ff55ff' : '#888888';
      }

      this.renderNetworkNodes(this.networkNodes);
    } catch (e) {
      console.warn('Failed to fetch network registry:', e);
    }
  }

  filterNetworkNodes(query) {
    const q = (query || '').toLowerCase().trim();
    if (!q) {
      this.renderNetworkNodes(this.networkNodes);
      return;
    }
    const filtered = this.networkNodes.filter(n => {
      return (n.callsign || '').toLowerCase().includes(q)
        || (n.name || '').toLowerCase().includes(q)
        || (n.description || '').toLowerCase().includes(q)
        || (n.location?.region || '').toLowerCase().includes(q)
        || (n.location?.grid || '').toLowerCase().includes(q);
    });
    this.renderNetworkNodes(filtered);
  }

  renderNetworkNodes(nodes) {
    const tbody = document.getElementById('network-nodes-tbody');
    if (!tbody) return;
    tbody.innerHTML = '';

    if (!nodes || nodes.length === 0) {
      tbody.innerHTML = '<tr><td colspan="7" class="empty-state" style="text-align: center; padding: 20px; color: #888;">No BBS nodes found in network registry.</td></tr>';
      return;
    }

    nodes.forEach(node => {
      const tr = document.createElement('tr');
      tr.style.borderBottom = '1px solid rgba(255,255,255,0.05)';

      const endpointsStr = (node.endpoints || []).map(ep => `${ep.protocol.toUpperCase()}://${ep.host}:${ep.port}`).join('<br>') || 'None';
      const appsStr = (node.capabilities?.supported_apps || []).join(', ') || 'Standard';

      const firstEndpoint = (node.endpoints && node.endpoints.length > 0) ? node.endpoints[0] : null;

      tr.innerHTML = `
        <td style="padding: 8px; font-weight: bold; color: #55ffff; font-family: monospace;">${escapeHtml(node.callsign || 'N/A')}</td>
        <td style="padding: 8px;">
          <strong>${escapeHtml(node.name || 'Unnamed BBS')}</strong>
          ${node.description ? `<div style="font-size: 11px; color: #888; margin-top: 2px;">${escapeHtml(node.description)}</div>` : ''}
        </td>
        <td style="padding: 8px;">
          <div>${escapeHtml(node.location?.region || 'Global')}</div>
          ${node.location?.grid ? `<span class="badge" style="font-size: 10px;">${escapeHtml(node.location.grid)}</span>` : ''}
        </td>
        <td style="padding: 8px; font-family: monospace; font-size: 11px;">${endpointsStr}</td>
        <td style="padding: 8px; font-size: 11px; color: #aaa;">${escapeHtml(appsStr)}</td>
        <td style="padding: 8px; font-size: 11px;">${escapeHtml(node.sysop?.contact || node.sysop?.handle || 'N/A')}</td>
        <td style="padding: 8px; text-align: right;">
          ${firstEndpoint ? `<button class="retro-btn btn-sm btn-ping-node" data-host="${escapeHtml(firstEndpoint.host)}" data-port="${firstEndpoint.port}">⚡ PING</button>` : ''}
        </td>
      `;

      tbody.appendChild(tr);
    });

    tbody.querySelectorAll('.btn-ping-node').forEach(btn => {
      btn.addEventListener('click', () => {
        this.testPingNode(btn.dataset.host, parseInt(btn.dataset.port, 10), btn);
      });
    });
  }

  async syncNetworkRegistry() {
    const btn = document.getElementById('btn-refresh-network');
    if (btn) {
      btn.disabled = true;
      btn.textContent = 'SYNCING...';
    }
    try {
      const res = await this.apiFetch('/api/network/sync', { method: 'POST' });
      if (res.ok) {
        const data = await res.json();
        alert(`Successfully synced network registry! ${data.synced_nodes || 0} nodes verified.`);
        await this.fetchNetworkRegistry();
      } else {
        alert('Registry sync failed: ' + (await res.text()));
      }
    } catch (e) {
      alert('Error syncing registry: ' + e);
    } finally {
      if (btn) {
        btn.disabled = false;
        btn.textContent = '↻ SYNC REGISTRY';
      }
    }
  }

  async testPingNode(host, port, btnEl) {
    const originalText = btnEl.textContent;
    btnEl.disabled = true;
    btnEl.textContent = '...';
    try {
      const res = await this.apiFetch('/api/network/test', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ host, port }),
      });
      if (res.ok) {
        const data = await res.json();
        if (data.reachable) {
          btnEl.textContent = `✓ ${data.latency_ms}ms`;
          btnEl.style.color = '#55ff55';
        } else {
          btnEl.textContent = `✗ FAIL`;
          btnEl.style.color = '#ff5555';
          console.warn('Ping failed for', host, port, data.error);
        }
      } else {
        btnEl.textContent = `ERR`;
      }
    } catch (e) {
      btnEl.textContent = `ERR`;
    } finally {
      setTimeout(() => {
        btnEl.disabled = false;
        btnEl.textContent = originalText;
        btnEl.style.color = '';
      }, 4000);
    }
  }

  // --- RADIO / KISS MODEM METHODS ---

  async fetchRadio() {
    try {
      const res = await this.apiFetch('/api/radio');
      if (!res.ok) return;
      const data = await res.json();

      const modeSelect = document.getElementById('radio-mode-select');
      const portInput = document.getElementById('radio-port-input');
      const baudInput = document.getElementById('radio-baud-input');
      const txPowerInput = document.getElementById('radio-txpower-input');
      const freqInput = document.getElementById('radio-freq-input');
      const bwSelect = document.getElementById('radio-bw-select');
      const sfSelect = document.getElementById('radio-sf-select');
      const crSelect = document.getElementById('radio-cr-select');

      if (modeSelect) modeSelect.value = data.mode || 'serial';
      if (portInput) portInput.value = data.port || '/dev/ttyACM1';
      if (baudInput) baudInput.value = data.baud_rate || 115200;
      if (txPowerInput) txPowerInput.value = data.tx_power_dbm !== undefined ? data.tx_power_dbm : 20;
      if (freqInput) freqInput.value = data.frequency_hz || 915000000;
      if (bwSelect && data.bandwidth_hz) bwSelect.value = String(data.bandwidth_hz);
      if (sfSelect && data.spreading_factor) sfSelect.value = String(data.spreading_factor);
      if (crSelect && data.coding_rate) crSelect.value = String(data.coding_rate);

      // Populate available serial ports datalist
      const portDatalist = document.getElementById('available-ports-list');
      if (portDatalist && Array.isArray(data.available_ports)) {
        portDatalist.innerHTML = '';
        data.available_ports.forEach(p => {
          const opt = document.createElement('option');
          opt.value = p;
          portDatalist.appendChild(opt);
        });
      }
    } catch (e) {
      console.warn('Radio fetch error:', e);
    }
  }

  async saveRadioConfig() {
    const mode = document.getElementById('radio-mode-select').value;
    const port = document.getElementById('radio-port-input').value;
    const baudRate = parseInt(document.getElementById('radio-baud-input').value, 10) || 115200;
    const txPower = parseInt(document.getElementById('radio-txpower-input').value, 10) || 20;
    const freq = parseInt(document.getElementById('radio-freq-input').value, 10) || 915000000;
    const bw = parseInt(document.getElementById('radio-bw-select').value, 10) || 250000;
    const sf = parseInt(document.getElementById('radio-sf-select').value, 10) || 7;
    const cr = parseInt(document.getElementById('radio-cr-select').value, 10) || 5;

    try {
      const res = await this.apiFetch('/api/radio', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          mode,
          port,
          baud_rate: baudRate,
          tx_power_dbm: txPower,
          frequency_hz: freq,
          bandwidth_hz: bw,
          spreading_factor: sf,
          coding_rate: cr,
        }),
      });

      if (res.ok) {
        alert('Radio hardware configuration saved! BBS daemon is restarting with new modem settings.');
        this.fetchRadio();
        this.fetchOverview();
      } else {
        alert('Failed to save radio configuration: ' + await res.text());
      }
    } catch (e) {
      alert('Error saving radio configuration: ' + e);
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
