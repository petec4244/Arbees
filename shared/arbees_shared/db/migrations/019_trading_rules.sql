-- Migration 019: Trading rules and loss analysis tables
-- Supports the loss analysis feedback loop for learning from losing trades

-- Trading rules table - stores automated blocking/threshold rules
CREATE TABLE IF NOT EXISTS trading_rules (
    rule_id VARCHAR(64) PRIMARY KEY,
    rule_type VARCHAR(32) NOT NULL,  -- block_pattern, threshold_override
    conditions JSONB NOT NULL,        -- {"sport": "NFL", "signal_type": "model_edge_yes", "edge_lt": 4.0}
    action JSONB NOT NULL,            -- {"type": "reject", "reason": "Pattern block"}
    source VARCHAR(32) DEFAULT 'automated',
    confidence DECIMAL(4,3),
    sample_size INTEGER,
    reason TEXT,
    status VARCHAR(16) DEFAULT 'active',  -- active, inactive, expired
    created_at TIMESTAMPTZ DEFAULT NOW(),
    expires_at TIMESTAMPTZ,
    match_count INTEGER DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_trading_rules_status ON trading_rules(status);
CREATE INDEX IF NOT EXISTS idx_trading_rules_expires ON trading_rules(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_trading_rules_type ON trading_rules(rule_type);

-- Loss analysis log - tracks root cause for each losing trade
CREATE TABLE IF NOT EXISTS loss_analysis (
    id SERIAL,
    trade_id VARCHAR(128) NOT NULL,
    root_cause VARCHAR(32) NOT NULL,  -- edge_too_thin, model_error, market_speed, etc
    sub_cause VARCHAR(64),
    confidence DECIMAL(4,3),
    sport VARCHAR(16),
    signal_type VARCHAR(32),
    edge_at_entry DECIMAL(6,3),
    details JSONB,
    analyzed_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (id, analyzed_at)
);

-- Convert to hypertable for time-series queries
SELECT create_hypertable('loss_analysis', 'analyzed_at', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_loss_analysis_trade ON loss_analysis(trade_id);
CREATE INDEX IF NOT EXISTS idx_loss_analysis_cause ON loss_analysis(root_cause, analyzed_at DESC);
CREATE INDEX IF NOT EXISTS idx_loss_analysis_sport ON loss_analysis(sport, signal_type, analyzed_at DESC);

-- Detected patterns table - aggregate patterns found across losing trades
CREATE TABLE IF NOT EXISTS detected_patterns (
    pattern_id VARCHAR(64) PRIMARY KEY,
    pattern_type VARCHAR(32) NOT NULL,  -- sport_signal, edge_bucket, timing_pattern
    pattern_key VARCHAR(256) UNIQUE NOT NULL,  -- e.g. "NFL:model_edge_yes" or "edge:<3%"
    description TEXT,
    sample_size INTEGER,
    loss_count INTEGER DEFAULT 0,
    win_rate DECIMAL(5,4),
    total_pnl DECIMAL(12,4),
    conditions JSONB,  -- Full condition set for matching
    suggested_action JSONB,  -- What the system suggests doing
    status VARCHAR(16) DEFAULT 'active',  -- active, dismissed, resolved
    first_detected_at TIMESTAMPTZ DEFAULT NOW(),
    last_updated_at TIMESTAMPTZ DEFAULT NOW(),
    rule_id VARCHAR(64) REFERENCES trading_rules(rule_id) ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS idx_detected_patterns_status ON detected_patterns(status);
CREATE INDEX IF NOT EXISTS idx_detected_patterns_type ON detected_patterns(pattern_type);
CREATE INDEX IF NOT EXISTS idx_detected_patterns_key ON detected_patterns(pattern_key);

-- Rule match log - tracks which signals were blocked by which rules
CREATE TABLE IF NOT EXISTS rule_matches (
    id SERIAL,
    rule_id VARCHAR(64) NOT NULL,
    signal_id VARCHAR(128),
    game_id VARCHAR(128),
    sport VARCHAR(16),
    signal_type VARCHAR(32),
    matched_at TIMESTAMPTZ DEFAULT NOW(),
    PRIMARY KEY (id, matched_at)
);

SELECT create_hypertable('rule_matches', 'matched_at', if_not_exists => TRUE);

CREATE INDEX IF NOT EXISTS idx_rule_matches_rule ON rule_matches(rule_id, matched_at DESC);
