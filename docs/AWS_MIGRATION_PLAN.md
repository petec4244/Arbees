# Arbees AWS Migration Plan

**Optimized for: Cost efficiency, rapid validation, incremental deployment**

**Strategy:** Validate locally → Fix critical blockers → Deploy incrementally to AWS → Scale as needed

---

## Executive Summary

### Current State
- ✅ 97.9% win rate in paper trading
- ✅ All infrastructure working locally
- ✅ Rust services containerized and cloud-ready
- ❌ **CRITICAL BLOCKER:** No real order execution (Polymarket not implemented)
- ⚠️ Running on local Windows PC with VPN

### Target State
- Multi-region AWS deployment (us-east-1 + eu-central-1)
- Real order execution on Kalshi and Polymarket
- 99.9% uptime with auto-restart
- Scalable to multiple strategies

### Timeline
- **Weeks 1-2:** Local validation + implement Polymarket execution
- **Week 3:** Decision point (stay local or migrate)
- **Week 4-5:** AWS deployment if justified
- **Week 6+:** Scaling and optimization

### Cost Analysis
| Phase | Monthly Cost | Justification |
|-------|--------------|---------------|
| Local validation | $17 | Test system stability before AWS spend |
| AWS (initial) | $260 | Full production deployment |
| Break-even | Daily profit >$50 | 1.6 days of trading pays for AWS |

---

## Phase 0: Local Validation (Weeks 1-2)

**Goal:** Prove system reliability and profitability before AWS investment

**Status:** ⭐ START HERE - CRITICAL PATH

### Why Local First?
1. ✅ Save $260/month during validation
2. ✅ Easier debugging (local logs, fast iteration)
3. ✅ No learning curve (you know Docker/Windows)
4. ✅ System already cloud-ready (easy to migrate later)
5. ✅ Validate 97.9% win rate wasn't a fluke

### 0.1: Implement Polymarket Execution (Priority: CRITICAL)

**Current Blocker:** `services/execution_service_rust/src/engine.rs:125-153` returns rejection for Polymarket

**What You'll Do Manually:**

#### Step 1: Research Polymarket CLOB API
```bash
# Review existing Python implementation (if any)
grep -r "py-clob-client" markets/

# Check terauss bot reference (mentioned in ARCHITECTURE_REVIEW)
# Location: External reference to Polymarket-Kalshi-Arbitrage-bot/src/polymarket_clob.rs
```

**Action Required:**
1. Decide on implementation approach:
   - **Option A:** Use `py-clob-client` Python library (faster, proven)
   - **Option B:** Port terauss Rust implementation (better performance, more work)
   - **Option C:** Use HTTP API directly (most control)

2. Document your choice and reasoning

#### Step 2: Implement Polymarket Execution Engine

**Manual Steps:**

1. **Create Polymarket client module** (if using Rust):
   ```bash
   # Create new module in rust_core
   touch rust_core/src/clients/polymarket_execution.rs
   ```

2. **Add to execution service:**
   - Open: [services/execution_service_rust/src/engine.rs](services/execution_service_rust/src/engine.rs#L125)
   - Replace rejection with real implementation:

   ```rust
   async fn execute_polymarket(&self, request: ExecutionRequest) -> Result<ExecutionResult> {
       // Check credentials
       if !self.polymarket.has_credentials() {
           return Ok(ExecutionResult {
               status: ExecutionStatus::Rejected,
               message: Some("Missing Polymarket credentials".to_string()),
               filled_quantity: 0.0,
               avg_fill_price: 0.0,
               fees: 0.0,
               order_id: None,
           });
       }

       // Convert side
       let side = match request.side {
           TradeSide::Buy => "BUY",
           TradeSide::Sell => "SELL",
       };

       // Place limit order via CLOB API
       match self.polymarket.place_limit_order(
           &request.market_id,
           side,
           request.limit_price,
           request.quantity,
       ).await {
           Ok(order) => {
               let filled_qty = order.filled_amount;
               let status = if filled_qty >= request.quantity {
                   ExecutionStatus::Filled
               } else if filled_qty > 0.0 {
                   ExecutionStatus::Partial
               } else {
                   ExecutionStatus::Pending
               };

               // Polymarket uses 2% taker fee
               let fees = calculate_fee(Platform::Polymarket, request.limit_price, filled_qty);

               Ok(ExecutionResult {
                   status,
                   message: Some(format!("Polymarket order {}", order.order_id)),
                   filled_quantity: filled_qty,
                   avg_fill_price: order.avg_price.unwrap_or(request.limit_price),
                   fees,
                   order_id: Some(order.order_id),
               })
           }
           Err(e) => Ok(ExecutionResult {
               status: ExecutionStatus::Rejected,
               message: Some(format!("Polymarket error: {}", e)),
               filled_quantity: 0.0,
               avg_fill_price: 0.0,
               fees: 0.0,
               order_id: None,
           }),
       }
   }
   ```

3. **Update fee calculation:**
   - Open: [services/execution_service_rust/src/engine.rs](services/execution_service_rust/src/engine.rs#L16)
   - Verify Polymarket fees:
   ```rust
   fn calculate_fee(platform: Platform, price: f64, quantity: f64) -> f64 {
       let notional = price * quantity;
       match platform {
           Platform::Kalshi => {
               // Kalshi: $1 flat fee for trades <$500
               if notional < 500.0 { 1.0 } else { notional * 0.01 }
           }
           Platform::Polymarket => {
               // Polymarket: 2% taker fee
               notional * 0.02
           }
       }
   }
   ```

4. **Test with small positions:**
   ```bash
   # Set paper trading OFF
   export PAPER_TRADING=0

   # Set small limits for testing
   export MAX_POSITION_SIZE=10.0
   export MAX_DAILY_LOSS=50.0

   # Rebuild and restart
   docker-compose --profile full build execution_service_rust
   docker-compose --profile full up -d

   # Monitor logs
   docker-compose logs -f execution_service_rust
   ```

**Validation Checklist:**
- [ ] Polymarket order placement works
- [ ] Fills are tracked correctly
- [ ] Fees are calculated accurately
- [ ] Position tracking updates
- [ ] P&L calculation is correct
- [ ] Tested with $10-20 positions for 24 hours

**Estimated Time:** 1-2 days (depending on API complexity)

---

### 0.2: Run Local Validation (2-3 weeks)

**Goal:** Prove system works 24/7 before AWS investment

**Manual Setup Steps:**

#### Step 1: Configure Local Environment for 24/7

1. **Disable Windows Updates:**
   ```
   Settings → Windows Update → Pause updates for 5 weeks
   ```

2. **Prevent Sleep:**
   ```
   Settings → System → Power & Sleep → Never
   ```

3. **Configure Docker to start on boot:**
   ```powershell
   # Run Docker Desktop on Windows startup
   # Settings → General → Start Docker Desktop when you log in
   ```

4. **Set up UPS (if available):**
   - Protects against power outages
   - Gives time to gracefully shut down if needed

#### Step 2: Configure Risk Limits

**Edit your `.env` file:**
```bash
# Conservative limits for validation
MAX_POSITION_SIZE=20.0        # Small positions initially
MAX_DAILY_LOSS=100.0          # Stop trading if lose $100/day
MIN_EDGE_PCT=5.0              # Higher threshold (was 2.0)
KELLY_FRACTION=0.10           # Conservative Kelly (was 0.25)
MAX_SIMULTANEOUS_POSITIONS=3  # Limit exposure

# Price staleness
PRICE_STALENESS_TTL=5         # 5 seconds max stale data

# Paper trading (turn OFF after Polymarket works)
PAPER_TRADING=0
```

**Restart services:**
```bash
docker-compose --profile full down
docker-compose --profile full up -d
```

#### Step 3: Daily Monitoring (Manual Task)

**Create a monitoring spreadsheet with:**

| Date | Uptime % | Trades | Wins | Losses | P&L | Issues | Manual Interventions |
|------|----------|--------|------|--------|-----|--------|---------------------|
| Day 1 | 98% | 15 | 14 | 1 | +$45 | VPN disconnect 2x | Restarted VPN container |
| Day 2 | 100% | 12 | 11 | 1 | +$38 | None | None |
| ... | | | | | | | |

**Daily checks (10 minutes/day):**
```bash
# Check service health
docker-compose ps

# Check recent trades
docker-compose exec timescaledb psql -U arbees -d arbees -c \
  "SELECT created_at, platform, side, quantity, entry_price, pnl
   FROM paper_trades
   WHERE created_at > NOW() - INTERVAL '24 hours'
   ORDER BY created_at DESC LIMIT 20;"

# Check current balance
docker-compose exec timescaledb psql -U arbees -d arbees -c \
  "SELECT * FROM bankroll ORDER BY timestamp DESC LIMIT 1;"

# Check for errors
docker-compose logs --tail=100 execution_service_rust | grep -i error
docker-compose logs --tail=100 game_shard_rust | grep -i error
```

**Red flags to watch for:**
- Win rate drops below 70%
- Frequent service restarts
- VPN disconnects more than 2x/day
- Manual interventions needed daily
- Missed trades due to downtime

#### Step 4: Week 2 Decision Point

**After 2 weeks, calculate metrics:**

```bash
# Total P&L
docker-compose exec timescaledb psql -U arbees -d arbees -c \
  "SELECT
     COUNT(*) as total_trades,
     SUM(CASE WHEN pnl > 0 THEN 1 ELSE 0 END) as wins,
     SUM(CASE WHEN pnl < 0 THEN 1 ELSE 0 END) as losses,
     SUM(pnl) as total_pnl,
     AVG(pnl) as avg_pnl,
     SUM(CASE WHEN pnl > 0 THEN pnl ELSE 0 END) as total_wins,
     SUM(CASE WHEN pnl < 0 THEN pnl ELSE 0 END) as total_losses
   FROM paper_trades
   WHERE created_at > NOW() - INTERVAL '14 days';"
```

**Decision Matrix:**

| Scenario | Daily Profit | Uptime | Decision |
|----------|--------------|--------|----------|
| A | >$100 | <95% | → **Deploy to AWS** (losing money to downtime) |
| B | >$50 | >95% | → Stay local or AWS (your choice) |
| C | >$50 | <95% | → **Deploy to AWS** (system works, needs reliability) |
| D | $20-50 | >95% | → **Stay local** (save $260/month) |
| E | $20-50 | <95% | → Fix uptime issues locally first |
| F | <$20 | Any | → **Stay local** (AWS not justified yet) |

**If deploying to AWS, proceed to Phase 1.**
**If staying local, skip to Phase 5 (optimization).**

---

## Phase 1: AWS Infrastructure Setup (Week 3-4)

**Goal:** Set up AWS foundation without deploying services yet

**Prerequisites:**
- [ ] Daily profit >$50 consistently
- [ ] System validated for 2+ weeks locally
- [ ] AWS account with admin access
- [ ] AWS CLI installed and configured

**Cost:** Initial infrastructure ~$28/month (RDS + ElastiCache only)

### 1.1: Create AWS Account Resources

**Manual Steps:**

#### Step 1: Install AWS CLI (if not already installed)

**Windows:**
```powershell
# Download and install AWS CLI v2
msiexec.exe /i https://awscli.amazonaws.com/AWSCLIV2.msi

# Verify installation
aws --version
```

#### Step 2: Configure AWS Credentials

```bash
# Run configuration wizard
aws configure

# Enter when prompted:
AWS Access Key ID: [Your access key]
AWS Secret Access Key: [Your secret key]
Default region name: us-east-1
Default output format: json
```

**Verify access:**
```bash
aws sts get-caller-identity
# Should show your account ID and user ARN
```

#### Step 3: Create VPC and Networking

**Option A: Using AWS Console (Easier)**

1. Open AWS Console → VPC
2. Click "Create VPC"
3. Select "VPC and more" (creates subnets automatically)
4. Configuration:
   - Name: `arbees-vpc`
   - IPv4 CIDR: `10.0.0.0/16`
   - Number of AZs: 2
   - Number of public subnets: 2
   - Number of private subnets: 2
   - NAT gateways: 1 (cheaper, single point of failure OK for now)
   - VPC endpoints: None (can add later)

5. Click "Create VPC"

**Option B: Using AWS CLI (Faster if you know what you're doing)**

```bash
# Create VPC
aws ec2 create-vpc --cidr-block 10.0.0.0/16 --tag-specifications 'ResourceType=vpc,Tags=[{Key=Name,Value=arbees-vpc}]'

# Save VPC ID from output
VPC_ID=vpc-xxxxx

# Create Internet Gateway
aws ec2 create-internet-gateway --tag-specifications 'ResourceType=internet-gateway,Tags=[{Key=Name,Value=arbees-igw}]'

# Attach to VPC
IGW_ID=igw-xxxxx
aws ec2 attach-internet-gateway --vpc-id $VPC_ID --internet-gateway-id $IGW_ID

# Create subnets (2 public, 2 private across 2 AZs)
aws ec2 create-subnet --vpc-id $VPC_ID --cidr-block 10.0.1.0/24 --availability-zone us-east-1a --tag-specifications 'ResourceType=subnet,Tags=[{Key=Name,Value=arbees-public-1a}]'
aws ec2 create-subnet --vpc-id $VPC_ID --cidr-block 10.0.2.0/24 --availability-zone us-east-1b --tag-specifications 'ResourceType=subnet,Tags=[{Key=Name,Value=arbees-public-1b}]'
aws ec2 create-subnet --vpc-id $VPC_ID --cidr-block 10.0.11.0/24 --availability-zone us-east-1a --tag-specifications 'ResourceType=subnet,Tags=[{Key=Name,Value=arbees-private-1a}]'
aws ec2 create-subnet --vpc-id $VPC_ID --cidr-block 10.0.12.0/24 --availability-zone us-east-1b --tag-specifications 'ResourceType=subnet,Tags=[{Key=Name,Value=arbees-private-1b}]'

# Create NAT Gateway (for private subnets to reach internet)
# Allocate Elastic IP first
aws ec2 allocate-address --domain vpc
EIP_ALLOC_ID=eipalloc-xxxxx

# Create NAT Gateway in public subnet
PUBLIC_SUBNET_1A=subnet-xxxxx
aws ec2 create-nat-gateway --subnet-id $PUBLIC_SUBNET_1A --allocation-id $EIP_ALLOC_ID --tag-specifications 'ResourceType=natgateway,Tags=[{Key=Name,Value=arbees-nat}]'
```

**Save these IDs for later:**
- VPC ID: `vpc-xxxxx`
- Public Subnet 1a: `subnet-xxxxx`
- Public Subnet 1b: `subnet-xxxxx`
- Private Subnet 1a: `subnet-xxxxx`
- Private Subnet 1b: `subnet-xxxxx`

---

### 1.2: Create RDS PostgreSQL with TimescaleDB

**Manual Steps:**

#### Step 1: Create DB Subnet Group

**AWS Console:**
1. RDS → Subnet groups → Create DB subnet group
2. Configuration:
   - Name: `arbees-db-subnet-group`
   - VPC: `arbees-vpc`
   - Add subnets: Select both **private** subnets (us-east-1a, us-east-1b)
3. Create

**AWS CLI:**
```bash
aws rds create-db-subnet-group \
  --db-subnet-group-name arbees-db-subnet-group \
  --db-subnet-group-description "Arbees database subnet group" \
  --subnet-ids subnet-xxxxx subnet-xxxxx \
  --tags Key=Name,Value=arbees-db-subnet-group
```

#### Step 2: Create Security Group for RDS

```bash
# Create security group
aws ec2 create-security-group \
  --group-name arbees-db-sg \
  --description "Security group for Arbees RDS" \
  --vpc-id $VPC_ID

# Save security group ID
DB_SG_ID=sg-xxxxx

# Allow PostgreSQL from VPC CIDR only
aws ec2 authorize-security-group-ingress \
  --group-id $DB_SG_ID \
  --protocol tcp \
  --port 5432 \
  --cidr 10.0.0.0/16
```

#### Step 3: Create RDS Instance

**AWS Console (Recommended - easier for first time):**

1. RDS → Databases → Create database
2. Configuration:
   - Engine: PostgreSQL
   - Version: 14.x (latest)
   - Template: Dev/Test (cheaper)
   - DB instance identifier: `arbees-db`
   - Master username: `arbees`
   - Master password: [Generate strong password - SAVE THIS!]
   - DB instance class: `db.t3.micro` (cheapest, ~$13/month)
   - Storage type: General Purpose SSD (gp3)
   - Allocated storage: 20 GB (can grow later)
   - Storage autoscaling: Enabled, max 100 GB
   - VPC: `arbees-vpc`
   - Subnet group: `arbees-db-subnet-group`
   - Public access: No
   - VPC security group: `arbees-db-sg`
   - Availability Zone: No preference
   - Database authentication: Password authentication
   - Initial database name: `arbees`
   - Backup: 7 days retention
   - Encryption: Enabled (default)
   - Enhanced monitoring: Disabled (save money)
   - Maintenance window: Preferred (e.g., Sun 03:00-04:00 AM)

3. Create database (takes ~10 minutes)

**Save the endpoint:** `arbees-db.xxxxx.us-east-1.rds.amazonaws.com`

#### Step 4: Enable TimescaleDB Extension

**Wait for RDS to be available, then connect:**

```bash
# Install PostgreSQL client if needed
# Windows: Download from https://www.postgresql.org/download/windows/

# Connect to RDS (replace endpoint and password)
psql -h arbees-db.xxxxx.us-east-1.rds.amazonaws.com -U arbees -d arbees

# Inside psql:
CREATE EXTENSION IF NOT EXISTS timescaledb;

# Verify
\dx

# Should show timescaledb extension
```

**Run migrations:**
```bash
# Copy migrations to a temp directory
mkdir temp_migrations
cp shared/arbees_shared/db/migrations/*.sql temp_migrations/

# Apply each migration
psql -h arbees-db.xxxxx.us-east-1.rds.amazonaws.com -U arbees -d arbees -f temp_migrations/001_initial_schema.sql
psql -h arbees-db.xxxxx.us-east-1.rds.amazonaws.com -U arbees -d arbees -f temp_migrations/002_timescale_hypertables.sql
# ... repeat for all migrations
```

**Verify tables exist:**
```sql
\dt

-- Should show: paper_trades, bankroll, game_states, market_prices, trading_signals, etc.
```

---

### 1.3: Create ElastiCache Redis

**Manual Steps:**

#### Step 1: Create Cache Subnet Group

**AWS Console:**
1. ElastiCache → Subnet groups → Create subnet group
2. Configuration:
   - Name: `arbees-cache-subnet-group`
   - VPC: `arbees-vpc`
   - Add subnets: Select both **private** subnets
3. Create

**AWS CLI:**
```bash
aws elasticache create-cache-subnet-group \
  --cache-subnet-group-name arbees-cache-subnet-group \
  --cache-subnet-group-description "Arbees cache subnet group" \
  --subnet-ids subnet-xxxxx subnet-xxxxx
```

#### Step 2: Create Security Group for Redis

```bash
# Create security group
aws ec2 create-security-group \
  --group-name arbees-redis-sg \
  --description "Security group for Arbees Redis" \
  --vpc-id $VPC_ID

# Save security group ID
REDIS_SG_ID=sg-xxxxx

# Allow Redis from VPC CIDR only
aws ec2 authorize-security-group-ingress \
  --group-id $REDIS_SG_ID \
  --protocol tcp \
  --port 6379 \
  --cidr 10.0.0.0/16
```

#### Step 3: Create Redis Cluster

**AWS Console (Recommended):**

1. ElastiCache → Redis clusters → Create Redis cluster
2. Configuration:
   - Cluster mode: Disabled (simpler)
   - Name: `arbees-redis`
   - Engine version: 7.x (latest)
   - Node type: `cache.t3.micro` (~$12/month)
   - Number of replicas: 0 (save money, can add later)
   - Subnet group: `arbees-cache-subnet-group`
   - Security groups: `arbees-redis-sg`
   - Encryption at rest: Disabled (not needed for non-sensitive data)
   - Encryption in transit: Disabled (within VPC)
   - Automatic backups: Disabled (Redis is cache, can rebuild)

3. Create (takes ~5 minutes)

**Save the endpoint:** `arbees-redis.xxxxx.cache.amazonaws.com:6379`

**Test connection:**
```bash
# Install redis-cli
# Windows: Download from https://github.com/microsoftarchive/redis/releases

# Test connection from local machine (won't work - private only)
# You'll test from ECS later

# Save endpoint for later use
echo "REDIS_URL=redis://arbees-redis.xxxxx.cache.amazonaws.com:6379" >> aws_endpoints.env
```

---

### 1.4: Create ECR Repositories

**Goal:** Push Docker images to AWS

**Manual Steps:**

#### Step 1: Create Repositories

```bash
# Create repository for each service
aws ecr create-repository --repository-name arbees/orchestrator-rust --region us-east-1
aws ecr create-repository --repository-name arbees/market-discovery-rust --region us-east-1
aws ecr create-repository --repository-name arbees/game-shard-rust --region us-east-1
aws ecr create-repository --repository-name arbees/signal-processor-rust --region us-east-1
aws ecr create-repository --repository-name arbees/execution-service-rust --region us-east-1
aws ecr create-repository --repository-name arbees/position-tracker-rust --region us-east-1
aws ecr create-repository --repository-name arbees/api --region us-east-1
aws ecr create-repository --repository-name arbees/frontend --region us-east-1

# Save repository URIs (will need for docker push)
# Format: {account-id}.dkr.ecr.us-east-1.amazonaws.com/arbees/{service}
```

#### Step 2: Login to ECR

```bash
# Get login password and login to Docker
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin {account-id}.dkr.ecr.us-east-1.amazonaws.com

# Should see: Login Succeeded
```

#### Step 3: Build and Push Images (DO NOT DO YET - just prepare)

**Create a helper script:** `scripts/push_to_ecr.sh`

```bash
#!/bin/bash

# Set your AWS account ID
ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
REGION=us-east-1
REGISTRY=$ACCOUNT_ID.dkr.ecr.$REGION.amazonaws.com

# Login
aws ecr get-login-password --region $REGION | docker login --username AWS --password-stdin $REGISTRY

# Build and push each service
services=(
  "orchestrator-rust:services/orchestrator_rust"
  "market-discovery-rust:services/market_discovery_rust"
  "game-shard-rust:services/game_shard_rust"
  "signal-processor-rust:services/signal_processor_rust"
  "execution-service-rust:services/execution_service_rust"
  "position-tracker-rust:services/position_tracker_rust"
  "api:services/api"
  "frontend:services/frontend"
)

for svc_path in "${services[@]}"; do
  IFS=':' read -r svc path <<< "$svc_path"
  echo "Building $svc..."
  docker build -t arbees/$svc -f $path/Dockerfile .
  docker tag arbees/$svc:latest $REGISTRY/arbees/$svc:latest
  docker push $REGISTRY/arbees/$svc:latest
done

echo "All images pushed to ECR!"
```

**Make executable:**
```bash
chmod +x scripts/push_to_ecr.sh
```

**Don't run yet - we'll push images in Phase 2 when deploying services.**

---

### 1.5: Store Secrets in AWS Secrets Manager

**Manual Steps:**

#### Step 1: Create Database Secret

```bash
# Create secret for database connection
aws secretsmanager create-secret \
  --name arbees/database-url \
  --description "PostgreSQL connection string for Arbees" \
  --secret-string "postgresql://arbees:{your-password}@arbees-db.xxxxx.us-east-1.rds.amazonaws.com:5432/arbees"

# Save ARN from output
DB_SECRET_ARN=arn:aws:secretsmanager:us-east-1:{account}:secret:arbees/database-url-xxxxx
```

#### Step 2: Create Redis Secret

```bash
aws secretsmanager create-secret \
  --name arbees/redis-url \
  --description "Redis connection string for Arbees" \
  --secret-string "redis://arbees-redis.xxxxx.cache.amazonaws.com:6379"

REDIS_SECRET_ARN=arn:aws:secretsmanager:us-east-1:{account}:secret:arbees/redis-url-xxxxx
```

#### Step 3: Create API Keys Secret

```bash
# Create JSON with all API keys
cat > api_keys.json <<EOF
{
  "KALSHI_EMAIL": "your-email@example.com",
  "KALSHI_PASSWORD": "your-password",
  "POLYMARKET_PRIVATE_KEY": "0x...",
  "ESPN_API_KEY": "your-espn-key-if-needed"
}
EOF

aws secretsmanager create-secret \
  --name arbees/api-keys \
  --description "API keys for external services" \
  --secret-string file://api_keys.json

API_KEYS_SECRET_ARN=arn:aws:secretsmanager:us-east-1:{account}:secret:arbees/api-keys-xxxxx

# Delete local file
rm api_keys.json
```

**Save all ARNs - you'll need them for ECS task definitions.**

---

### Phase 1 Completion Checklist

- [ ] VPC created with 2 public and 2 private subnets
- [ ] NAT Gateway configured for private subnet internet access
- [ ] RDS PostgreSQL created with TimescaleDB extension
- [ ] All database migrations applied
- [ ] ElastiCache Redis cluster created and accessible
- [ ] ECR repositories created for all 8 services
- [ ] Secrets stored in Secrets Manager
- [ ] Endpoints documented in `aws_endpoints.env`

**Infrastructure cost at this point:** ~$28/month (RDS + Redis + NAT Gateway)

**Next:** Phase 2 - Deploy services to ECS

---

## Phase 2: Deploy Core Services to ECS (Week 4)

**Goal:** Deploy trading services incrementally, validate each step

**Strategy:** Deploy in dependency order, test after each service

### 2.1: Create ECS Cluster

**Manual Steps:**

#### Step 1: Create ECS Cluster

**AWS Console:**
1. ECS → Clusters → Create cluster
2. Configuration:
   - Cluster name: `arbees-cluster`
   - Infrastructure: AWS Fargate (serverless)
   - Monitoring: Container Insights (disable to save money, enable later if needed)
3. Create

**AWS CLI:**
```bash
aws ecs create-cluster --cluster-name arbees-cluster --region us-east-1
```

#### Step 2: Create CloudWatch Log Group

```bash
# Create log group for all services
aws logs create-log-group --log-group-name /ecs/arbees --region us-east-1

# Set retention (7 days to save money)
aws logs put-retention-policy --log-group-name /ecs/arbees --retention-in-days 7
```

---

### 2.2: Create ECS Task Execution Role

**Manual Steps:**

#### Step 1: Create IAM Role

**Create trust policy file:** `ecs-task-trust-policy.json`
```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Principal": {
        "Service": "ecs-tasks.amazonaws.com"
      },
      "Action": "sts:AssumeRole"
    }
  ]
}
```

**Create role:**
```bash
aws iam create-role \
  --role-name arbees-ecs-execution-role \
  --assume-role-policy-document file://ecs-task-trust-policy.json

# Attach AWS managed policy for ECS task execution
aws iam attach-role-policy \
  --role-name arbees-ecs-execution-role \
  --policy-arn arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy

# Attach policy for Secrets Manager access
aws iam attach-role-policy \
  --role-name arbees-ecs-execution-role \
  --policy-arn arn:aws:iam::aws:policy/SecretsManagerReadWrite
```

**Save role ARN:**
```
arn:aws:iam::{account-id}:role/arbees-ecs-execution-role
```

---

### 2.3: Deploy Services (Incremental Approach)

**Deployment Order (dependency-first):**
1. orchestrator-rust (discovers games)
2. market-discovery-rust (finds market IDs)
3. game-shard-rust (monitors games)
4. signal-processor-rust (generates signals)
5. execution-service-rust (executes trades)
6. position-tracker-rust (tracks positions)

**For each service, follow this pattern:**

#### Service Deployment Template

**Step 1: Build and push Docker image**
```bash
# Example for orchestrator-rust
docker build -t arbees/orchestrator-rust -f services/orchestrator_rust/Dockerfile .

# Tag for ECR
docker tag arbees/orchestrator-rust:latest \
  {account-id}.dkr.ecr.us-east-1.amazonaws.com/arbees/orchestrator-rust:latest

# Push to ECR
docker push {account-id}.dkr.ecr.us-east-1.amazonaws.com/arbees/orchestrator-rust:latest
```

**Step 2: Create task definition**

**Create file:** `task-definitions/orchestrator-rust.json`

```json
{
  "family": "arbees-orchestrator-rust",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"],
  "cpu": "256",
  "memory": "512",
  "executionRoleArn": "arn:aws:iam::{account-id}:role/arbees-ecs-execution-role",
  "containerDefinitions": [
    {
      "name": "orchestrator-rust",
      "image": "{account-id}.dkr.ecr.us-east-1.amazonaws.com/arbees/orchestrator-rust:latest",
      "essential": true,
      "logConfiguration": {
        "logDriver": "awslogs",
        "options": {
          "awslogs-group": "/ecs/arbees",
          "awslogs-region": "us-east-1",
          "awslogs-stream-prefix": "orchestrator"
        }
      },
      "environment": [
        {"name": "RUST_LOG", "value": "info"},
        {"name": "SERVICE_NAME", "value": "orchestrator"},
        {"name": "MIN_EDGE_PCT", "value": "5.0"},
        {"name": "DISCOVERY_INTERVAL_SECS", "value": "300"}
      ],
      "secrets": [
        {
          "name": "DATABASE_URL",
          "valueFrom": "arn:aws:secretsmanager:us-east-1:{account}:secret:arbees/database-url-xxxxx"
        },
        {
          "name": "REDIS_URL",
          "valueFrom": "arn:aws:secretsmanager:us-east-1:{account}:secret:arbees/redis-url-xxxxx"
        }
      ]
    }
  ]
}
```

**Register task definition:**
```bash
aws ecs register-task-definition --cli-input-json file://task-definitions/orchestrator-rust.json
```

**Step 3: Create ECS service**

```bash
aws ecs create-service \
  --cluster arbees-cluster \
  --service-name orchestrator-rust \
  --task-definition arbees-orchestrator-rust \
  --desired-count 1 \
  --launch-type FARGATE \
  --network-configuration "awsvpcConfiguration={subnets=[subnet-xxxxx,subnet-xxxxx],securityGroups=[sg-xxxxx],assignPublicIp=DISABLED}" \
  --region us-east-1
```

**Step 4: Verify deployment**

```bash
# Check service status
aws ecs describe-services \
  --cluster arbees-cluster \
  --services orchestrator-rust \
  --region us-east-1

# Check logs
aws logs tail /ecs/arbees --follow --since 5m --filter-pattern "orchestrator"
```

**Step 5: Validate functionality**

```bash
# Check Redis for game discoveries
redis-cli -h arbees-redis.xxxxx.cache.amazonaws.com
> KEYS games:*
> KEYS discovery:*

# Should see game IDs being published
```

**Repeat for each service using the appropriate CPU/memory:**

| Service | CPU | Memory | Notes |
|---------|-----|--------|-------|
| orchestrator-rust | 256 | 512 | Low frequency |
| market-discovery-rust | 256 | 512 | RPC server |
| game-shard-rust | 512 | 1024 | Multiple instances (1 per 10 games) |
| signal-processor-rust | 256 | 512 | Signal validation |
| execution-service-rust | 512 | 1024 | Latency-critical |
| position-tracker-rust | 256 | 512 | Position monitoring |

---

### 2.4: Handle VPN for Polymarket (Hybrid Approach)

**Problem:** AWS Fargate doesn't support VPN containers (requires `NET_ADMIN` capability)

**Solution:** Deploy polymarket_monitor on **EC2 instance** with gluetun VPN

**Manual Steps:**

#### Step 1: Launch EC2 Instance in EU Region

**Why EU?** Polymarket geo-restrictions apply to US IPs, but not EU IPs. Deploy directly in eu-central-1 to avoid needing VPN.

**AWS Console:**
1. EC2 → Launch instance
2. Configuration:
   - Name: `arbees-polymarket-monitor`
   - Region: **eu-central-1** (Frankfurt)
   - AMI: Amazon Linux 2023
   - Instance type: `t3.micro` (~$7/month)
   - Key pair: Create new or use existing (for SSH access)
   - Network: Create new VPC or use default
   - Security group: Allow SSH (22) from your IP only
   - Storage: 8 GB gp3

3. Launch instance

#### Step 2: Install Docker on EC2

**SSH into instance:**
```bash
ssh -i your-key.pem ec2-user@{ec2-public-ip}
```

**Inside EC2:**
```bash
# Update system
sudo yum update -y

# Install Docker
sudo yum install -y docker
sudo systemctl start docker
sudo systemctl enable docker
sudo usermod -a -G docker ec2-user

# Logout and login again to apply group changes
exit
ssh -i your-key.pem ec2-user@{ec2-public-ip}

# Verify Docker works
docker ps
```

#### Step 3: Deploy Polymarket Monitor Container

**Create docker-compose file on EC2:**
```bash
mkdir arbees
cd arbees
nano docker-compose.yml
```

**Contents:**
```yaml
version: '3.8'

services:
  polymarket_monitor:
    image: {account-id}.dkr.ecr.us-east-1.amazonaws.com/arbees/polymarket-monitor:latest
    restart: unless-stopped
    environment:
      - REDIS_URL=${REDIS_URL}
      - RUST_LOG=info
      - SERVICE_NAME=polymarket_monitor
    # No VPN needed in EU region!
```

**Set environment variables:**
```bash
export REDIS_URL="redis://arbees-redis.xxxxx.cache.amazonaws.com:6379"
```

**Pull image from ECR:**
```bash
# Login to ECR from EC2
aws ecr get-login-password --region us-east-1 | docker login --username AWS --password-stdin {account-id}.dkr.ecr.us-east-1.amazonaws.com

# Start service
docker-compose up -d

# Check logs
docker-compose logs -f
```

**Note:** No VPN needed because EC2 is in EU region where Polymarket is accessible!

**Cost:** ~$7/month for t3.micro

---

### Phase 2 Completion Checklist

- [ ] ECS cluster created
- [ ] All 6 Rust services deployed to ECS Fargate
- [ ] CloudWatch logs streaming for all services
- [ ] Polymarket monitor running on EC2 in eu-central-1
- [ ] Services can connect to RDS and Redis
- [ ] Game discoveries appearing in Redis
- [ ] Market IDs being matched
- [ ] Signals being generated
- [ ] Trades being executed (paper or live)

**Cost at this point:** ~$260/month

**Validation:** Run for 48 hours, monitor for errors, verify trades executing correctly

---

## Phase 3: Deploy Frontend/API (Optional - Week 5)

**Goal:** Access trading dashboard from anywhere

**Decision:** Deploy to AWS or keep running locally?

### Option A: Deploy to AWS (Recommended if you travel)

**Benefits:**
- Access dashboard from anywhere
- Professional setup
- Integrated with AWS services

**Cost:** +$25/month (ALB + Fargate for API/frontend)

**Steps:**

1. Create Application Load Balancer (ALB)
2. Deploy API service to ECS
3. Deploy frontend to ECS or S3 + CloudFront
4. Configure HTTPS with ACM certificate
5. Set up Route53 domain (optional)

**Not covering detailed steps here to keep doc focused. Add if you want public access.**

---

### Option B: Keep Local (Recommended initially)

**Benefits:**
- Save $25/month
- Easy access from home network
- Can SSH tunnel to AWS if needed

**Setup:**

```bash
# Update .env to point to AWS RDS/Redis
DATABASE_URL=postgresql://arbees:{password}@arbees-db.xxxxx.us-east-1.rds.amazonaws.com:5432/arbees
REDIS_URL=redis://arbees-redis.xxxxx.cache.amazonaws.com:6379

# Allow your IP in security groups
aws ec2 authorize-security-group-ingress \
  --group-id $DB_SG_ID \
  --protocol tcp \
  --port 5432 \
  --cidr {your-public-ip}/32

aws ec2 authorize-security-group-ingress \
  --group-id $REDIS_SG_ID \
  --protocol tcp \
  --port 6379 \
  --cidr {your-public-ip}/32

# Run frontend locally
docker-compose up -d api frontend

# Access at http://localhost:3000
```

**Recommendation:** Keep local initially, deploy to AWS later if needed.

---

## Phase 4: Monitoring & Optimization (Ongoing)

**Goal:** Ensure system is healthy, optimize costs

### 4.1: Set Up CloudWatch Alarms

**Manual Steps:**

#### Critical Alarms (Set these first)

**1. Service CPU High:**
```bash
aws cloudwatch put-metric-alarm \
  --alarm-name arbees-execution-cpu-high \
  --alarm-description "Execution service CPU >80%" \
  --metric-name CPUUtilization \
  --namespace AWS/ECS \
  --statistic Average \
  --period 300 \
  --threshold 80 \
  --comparison-operator GreaterThanThreshold \
  --evaluation-periods 2 \
  --dimensions Name=ServiceName,Value=execution-service-rust Name=ClusterName,Value=arbees-cluster
```

**2. Service Task Count:**
```bash
aws cloudwatch put-metric-alarm \
  --alarm-name arbees-execution-no-tasks \
  --alarm-description "Execution service has 0 running tasks" \
  --metric-name RunningTaskCount \
  --namespace ECS/ContainerInsights \
  --statistic Average \
  --period 60 \
  --threshold 1 \
  --comparison-operator LessThanThreshold \
  --evaluation-periods 3 \
  --dimensions Name=ServiceName,Value=execution-service-rust Name=ClusterName,Value=arbees-cluster
```

**3. RDS CPU High:**
```bash
aws cloudwatch put-metric-alarm \
  --alarm-name arbees-db-cpu-high \
  --metric-name CPUUtilization \
  --namespace AWS/RDS \
  --statistic Average \
  --period 300 \
  --threshold 80 \
  --comparison-operator GreaterThanThreshold \
  --evaluation-periods 2 \
  --dimensions Name=DBInstanceIdentifier,Value=arbees-db
```

**4. Redis Memory High:**
```bash
aws cloudwatch put-metric-alarm \
  --alarm-name arbees-redis-memory-high \
  --metric-name DatabaseMemoryUsagePercentage \
  --namespace AWS/ElastiCache \
  --statistic Average \
  --period 300 \
  --threshold 90 \
  --comparison-operator GreaterThanThreshold \
  --evaluation-periods 2 \
  --dimensions Name=CacheClusterId,Value=arbees-redis
```

**Set up SNS topic for alerts:**
```bash
# Create SNS topic
aws sns create-topic --name arbees-alerts

# Subscribe your email
aws sns subscribe \
  --topic-arn arn:aws:sns:us-east-1:{account}:arbees-alerts \
  --protocol email \
  --notification-endpoint your-email@example.com

# Confirm subscription via email

# Add SNS topic to all alarms using --alarm-actions
```

---

### 4.2: Cost Optimization

**Daily Monitoring (First Month):**

```bash
# Check AWS costs daily
aws ce get-cost-and-usage \
  --time-period Start=2024-01-01,End=2024-01-31 \
  --granularity DAILY \
  --metrics BlendedCost \
  --group-by Type=SERVICE

# Expected breakdown:
# - RDS: $13/month
# - ElastiCache: $12/month
# - Fargate: $150-180/month
# - EC2 (Polymarket): $7/month
# - NAT Gateway: $32/month
# - Data transfer: $10-20/month
# Total: ~$260/month
```

**Optimization Tips:**

1. **Right-size Fargate tasks:**
   - Monitor CPU/memory usage after 1 week
   - Reduce if <50% utilized
   - Example: orchestrator might only need 128 CPU, 256 MB

2. **Use Spot instances for non-critical services:**
   - Save 70% on EC2 costs
   - Good for: analytics, archiver, futures_monitor

3. **Reduce log retention:**
   - 7 days for debug logs
   - 30 days for trade logs
   - Saves $5-10/month

4. **Consider reserved instances after 3 months:**
   - If system is stable and profitable
   - 1-year reserved RDS saves ~30%

---

### 4.3: Performance Monitoring

**Create custom dashboard:**

**AWS Console:**
1. CloudWatch → Dashboards → Create dashboard
2. Name: `arbees-trading`
3. Add widgets:
   - Line chart: ECS CPU utilization (all services)
   - Line chart: RDS connections
   - Line chart: Redis memory usage
   - Number: Running tasks count
   - Logs: Recent errors (filter pattern: `ERROR|WARN`)

**Monitor these metrics weekly:**

| Metric | Target | Action if exceeded |
|--------|--------|-------------------|
| Service CPU | <70% | Optimize code or add more CPU |
| Service Memory | <80% | Optimize or add memory |
| RDS connections | <50 | Add connection pooling |
| Redis memory | <75% | Review data TTLs |
| P&L variance | ±20% of expected | Investigate model drift |

---

## Phase 5: Scaling & Advanced Features (Month 2+)

**Only implement after system is stable and profitable**

### 5.1: Auto-scaling

**Set up auto-scaling for game shards:**

```bash
# Create scaling target
aws application-autoscaling register-scalable-target \
  --service-namespace ecs \
  --scalable-dimension ecs:service:DesiredCount \
  --resource-id service/arbees-cluster/game-shard-rust \
  --min-capacity 1 \
  --max-capacity 5

# Create scaling policy (scale up when CPU >70%)
aws application-autoscaling put-scaling-policy \
  --service-namespace ecs \
  --scalable-dimension ecs:service:DesiredCount \
  --resource-id service/arbees-cluster/game-shard-rust \
  --policy-name cpu-scaling \
  --policy-type TargetTrackingScaling \
  --target-tracking-scaling-policy-configuration file://scaling-policy.json
```

**scaling-policy.json:**
```json
{
  "TargetValue": 70.0,
  "PredefinedMetricSpecification": {
    "PredefinedMetricType": "ECSServiceAverageCPUUtilization"
  },
  "ScaleInCooldown": 300,
  "ScaleOutCooldown": 60
}
```

---

### 5.2: Multi-Strategy Support

**When to add:**
- Daily profit consistently >$200
- Single strategy is capital-constrained
- Want to test new sports/models

**Steps:**
1. Deploy additional game shard instances with different MIN_EDGE_PCT
2. Deploy separate signal processors with different strategies
3. Implement strategy-level position tracking
4. Add strategy performance dashboard

---

### 5.3: Advanced Optimizations

**Implement when:**
- System is stable for 1+ month
- Daily profit >$500
- Have spare time for optimization

**Ideas:**
1. **Move to Rust entirely:**
   - Port signal_processor to Rust
   - Port position_tracker to Rust
   - Expected 30% latency reduction

2. **Add caching layer:**
   - Cache ESPN game data (reduce API calls)
   - Cache Kalshi prices (reduce latency)
   - Use Redis for hot data

3. **Optimize database:**
   - Add indexes for slow queries
   - Partition large tables
   - Use materialized views

4. **Add machine learning:**
   - Train model on historical data
   - Predict price movements
   - Dynamic edge threshold

---

## Rollback Plan

**If AWS deployment fails or costs exceed profit:**

### Quick Rollback to Local

```bash
# Stop all ECS services
aws ecs update-service --cluster arbees-cluster --service orchestrator-rust --desired-count 0
aws ecs update-service --cluster arbees-cluster --service market-discovery-rust --desired-count 0
aws ecs update-service --cluster arbees-cluster --service game-shard-rust --desired-count 0
aws ecs update-service --cluster arbees-cluster --service signal-processor-rust --desired-count 0
aws ecs update-service --cluster arbees-cluster --service execution-service-rust --desired-count 0
aws ecs update-service --cluster arbees-cluster --service position-tracker-rust --desired-count 0

# Restart local Docker
docker-compose --profile full up -d

# Services will reconnect to AWS RDS/Redis (faster than re-migrating data)
```

**Keep AWS infrastructure running:** RDS + Redis only costs $28/month and preserves all your data.

---

## Cost Summary

### Local (Current)
- Electricity: $5/month
- VPN: $12/month
- **Total: $17/month**

### AWS (Phase 1 - Infrastructure Only)
- RDS: $13/month
- ElastiCache: $12/month
- NAT Gateway: $3/month
- **Total: $28/month**

### AWS (Phase 2 - Full Deployment)
- RDS: $13/month
- ElastiCache: $12/month
- NAT Gateway: $32/month
- Fargate (6 services): $150/month
- EC2 (Polymarket): $7/month
- Data transfer: $15/month
- **Total: $260/month**

### Break-even Analysis
| Daily Profit | Monthly Profit | AWS Cost | Net Profit | Break-even |
|--------------|----------------|----------|------------|------------|
| $20 | $600 | $260 | $340 | 13 days |
| $50 | $1,500 | $260 | $1,240 | 5.2 days |
| $100 | $3,000 | $260 | $2,740 | 2.6 days |
| $200 | $6,000 | $260 | $5,740 | 1.3 days |

**Conclusion:** If daily profit >$50, AWS pays for itself in <6 days.

---

## Timeline Summary

| Phase | Duration | Cost | Deliverable |
|-------|----------|------|-------------|
| Phase 0: Local validation | 2-3 weeks | $17/mo | Proven system |
| Phase 1: AWS infrastructure | 3-5 days | $28/mo | RDS + Redis ready |
| Phase 2: Deploy services | 3-5 days | $260/mo | Full AWS deployment |
| Phase 3: Frontend (optional) | 2-3 days | +$25/mo | Public dashboard |
| Phase 4: Monitoring | 1 day | $0 | Alerts + dashboards |
| Phase 5: Scaling | Ongoing | Variable | Multi-strategy |

**Total time to production AWS:** 4-6 weeks from start

**Critical path:** Polymarket execution → Local validation → AWS deployment

---

## Decision Checkpoints

### Checkpoint 1: After Week 1
**Questions:**
- Is Polymarket execution working?
- Are trades executing correctly?
- Is win rate sustained?

**Decide:** Continue validation or fix issues

---

### Checkpoint 2: After Week 2
**Questions:**
- Is daily profit >$50?
- Is uptime >95%?
- Are manual interventions needed?

**Decide:**
- Stay local (if uptime good, profit <$50)
- Deploy to AWS (if profit >$50 or uptime issues)
- Fix issues (if neither works well)

---

### Checkpoint 3: After AWS Deployment (Week 4-5)
**Questions:**
- Are AWS costs as expected ($260/month)?
- Is latency better or worse than local?
- Is profit covering AWS costs?

**Decide:**
- Continue on AWS (if profitable)
- Rollback to local (if costs exceed profit)
- Optimize (if borderline)

---

## Next Steps

**Right now:**
1. Read this entire document
2. Decide if you're ready to commit 4-6 weeks to this migration
3. If yes, start with Phase 0.1 (Implement Polymarket execution)
4. If no, stay on local until you have more confidence

**This week:**
- [ ] Implement Polymarket execution
- [ ] Test with small positions ($10-20)
- [ ] Start 24/7 local validation

**Week 2:**
- [ ] Continue monitoring
- [ ] Calculate daily profit average
- [ ] Make AWS decision

**Week 3-4 (if deploying to AWS):**
- [ ] Set up AWS infrastructure (Phase 1)
- [ ] Deploy services incrementally (Phase 2)
- [ ] Validate on AWS with small positions

**Week 5+:**
- [ ] Scale up position sizes
- [ ] Monitor costs vs profit
- [ ] Optimize as needed

---

## Support & Troubleshooting

### Common Issues

**Issue: Fargate task failing to start**
- Check CloudWatch logs: `aws logs tail /ecs/arbees --follow`
- Verify secrets ARNs are correct in task definition
- Check security group allows traffic

**Issue: Can't connect to RDS from local**
- Verify security group allows your IP
- Test with psql: `psql -h {endpoint} -U arbees -d arbees`
- Check VPC routing

**Issue: High AWS costs**
- Review Cost Explorer daily
- Right-size Fargate tasks (reduce CPU/memory)
- Reduce log retention
- Use Spot instances for non-critical services

**Issue: Low profitability on AWS**
- Check latency metrics (might be worse than local)
- Review win rate (should be same or better)
- Verify all services are running correctly
- Consider rollback to local

---

## Appendix: Manual Checklist

**Phase 0: Local Validation**
- [ ] Implement Polymarket execution
- [ ] Test with $10-20 positions
- [ ] Configure Windows for 24/7 operation
- [ ] Set conservative risk limits
- [ ] Monitor daily for 2 weeks
- [ ] Calculate average daily profit
- [ ] Make AWS decision

**Phase 1: AWS Infrastructure**
- [ ] Install and configure AWS CLI
- [ ] Create VPC with subnets
- [ ] Create NAT Gateway
- [ ] Create RDS PostgreSQL instance
- [ ] Enable TimescaleDB extension
- [ ] Run database migrations
- [ ] Create ElastiCache Redis cluster
- [ ] Create ECR repositories
- [ ] Store secrets in Secrets Manager
- [ ] Document all endpoints

**Phase 2: Deploy Services**
- [ ] Create ECS cluster
- [ ] Create CloudWatch log group
- [ ] Create IAM execution role
- [ ] Build and push all Docker images
- [ ] Create task definitions for each service
- [ ] Deploy orchestrator-rust
- [ ] Deploy market-discovery-rust
- [ ] Deploy game-shard-rust
- [ ] Deploy signal-processor-rust
- [ ] Deploy execution-service-rust
- [ ] Deploy position-tracker-rust
- [ ] Deploy polymarket_monitor on EC2 (eu-central-1)
- [ ] Verify all services are running
- [ ] Test end-to-end trade execution

**Phase 3: Monitoring**
- [ ] Set up CloudWatch alarms
- [ ] Create SNS topic for alerts
- [ ] Create CloudWatch dashboard
- [ ] Monitor costs daily
- [ ] Review logs for errors

**Phase 4: Validation**
- [ ] Run on AWS for 48 hours
- [ ] Verify trades executing correctly
- [ ] Compare performance to local
- [ ] Calculate actual AWS costs
- [ ] Decide: continue on AWS or rollback

---

**This is your complete migration plan. Start with Phase 0 and work through systematically. Good luck!** 🚀
