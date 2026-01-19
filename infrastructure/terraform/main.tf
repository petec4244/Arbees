# Arbees Infrastructure - AWS Multi-Region Deployment
#
# US Region (us-east-1): Kalshi/Core services
# EU Region (eu-central-1): Polymarket proxy

terraform {
  required_version = ">= 1.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket         = "arbees-terraform-state"
    key            = "infrastructure/terraform.tfstate"
    region         = "us-east-1"
    encrypt        = true
    dynamodb_table = "arbees-terraform-locks"
  }
}

# US-East-1 Provider (Primary)
provider "aws" {
  region = "us-east-1"
  alias  = "us"

  default_tags {
    tags = {
      Project     = "arbees"
      Environment = var.environment
      ManagedBy   = "terraform"
    }
  }
}

# EU-Central-1 Provider (Polymarket)
provider "aws" {
  region = "eu-central-1"
  alias  = "eu"

  default_tags {
    tags = {
      Project     = "arbees"
      Environment = var.environment
      ManagedBy   = "terraform"
    }
  }
}

# ==============================================================================
# Variables
# ==============================================================================

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "prod"
}

variable "vpc_cidr_us" {
  description = "CIDR block for US VPC"
  type        = string
  default     = "10.0.0.0/16"
}

variable "vpc_cidr_eu" {
  description = "CIDR block for EU VPC"
  type        = string
  default     = "10.1.0.0/16"
}

variable "db_instance_class" {
  description = "RDS instance class"
  type        = string
  default     = "db.t3.medium"
}

variable "redis_node_type" {
  description = "ElastiCache node type"
  type        = string
  default     = "cache.t3.small"
}

# ==============================================================================
# US Region Resources
# ==============================================================================

module "us_vpc" {
  source = "./modules/vpc"
  providers = {
    aws = aws.us
  }

  name             = "arbees-us-${var.environment}"
  cidr             = var.vpc_cidr_us
  azs              = ["us-east-1a", "us-east-1b", "us-east-1c"]
  private_subnets  = ["10.0.1.0/24", "10.0.2.0/24", "10.0.3.0/24"]
  public_subnets   = ["10.0.101.0/24", "10.0.102.0/24", "10.0.103.0/24"]
  database_subnets = ["10.0.201.0/24", "10.0.202.0/24", "10.0.203.0/24"]
}

module "us_ecs" {
  source = "./modules/ecs"
  providers = {
    aws = aws.us
  }

  cluster_name = "arbees-us-${var.environment}"
  vpc_id       = module.us_vpc.vpc_id
  subnet_ids   = module.us_vpc.private_subnet_ids

  services = {
    orchestrator = {
      cpu               = 512
      memory            = 1024
      desired_count     = 1
      container_port    = 8080
      health_check_path = "/health"
    }
    game_shard = {
      cpu               = 1024
      memory            = 2048
      desired_count     = 3
      container_port    = 8080
      health_check_path = "/health"
    }
    api = {
      cpu               = 512
      memory            = 1024
      desired_count     = 2
      container_port    = 8000
      health_check_path = "/api/monitoring/health"
    }
  }
}

module "us_database" {
  source = "./modules/rds"
  providers = {
    aws = aws.us
  }

  identifier     = "arbees-timescaledb-${var.environment}"
  engine         = "postgres"
  engine_version = "15"
  instance_class = var.db_instance_class
  storage_size   = 100

  vpc_id     = module.us_vpc.vpc_id
  subnet_ids = module.us_vpc.database_subnet_ids

  database_name = "arbees"
  username      = "arbees"
}

module "us_redis" {
  source = "./modules/elasticache"
  providers = {
    aws = aws.us
  }

  cluster_id = "arbees-redis-${var.environment}"
  node_type  = var.redis_node_type
  num_nodes  = 2

  vpc_id     = module.us_vpc.vpc_id
  subnet_ids = module.us_vpc.private_subnet_ids
}

module "us_alb" {
  source = "./modules/alb"
  providers = {
    aws = aws.us
  }

  name       = "arbees-alb-${var.environment}"
  vpc_id     = module.us_vpc.vpc_id
  subnet_ids = module.us_vpc.public_subnet_ids

  targets = {
    api = {
      port        = 8000
      target_type = "ip"
    }
  }
}

# ==============================================================================
# EU Region Resources (Polymarket Proxy)
# ==============================================================================

module "eu_vpc" {
  source = "./modules/vpc"
  providers = {
    aws = aws.eu
  }

  name             = "arbees-eu-${var.environment}"
  cidr             = var.vpc_cidr_eu
  azs              = ["eu-central-1a", "eu-central-1b"]
  private_subnets  = ["10.1.1.0/24", "10.1.2.0/24"]
  public_subnets   = ["10.1.101.0/24", "10.1.102.0/24"]
  database_subnets = []
}

module "eu_ecs" {
  source = "./modules/ecs"
  providers = {
    aws = aws.eu
  }

  cluster_name = "arbees-eu-${var.environment}"
  vpc_id       = module.eu_vpc.vpc_id
  subnet_ids   = module.eu_vpc.private_subnet_ids

  services = {
    polymarket_proxy = {
      cpu               = 256
      memory            = 512
      desired_count     = 2
      container_port    = 8080
      health_check_path = "/health"
    }
  }
}

# ==============================================================================
# VPC Peering (US <-> EU)
# ==============================================================================

resource "aws_vpc_peering_connection" "us_to_eu" {
  provider = aws.us

  vpc_id        = module.us_vpc.vpc_id
  peer_vpc_id   = module.eu_vpc.vpc_id
  peer_region   = "eu-central-1"
  auto_accept   = false

  tags = {
    Name = "arbees-us-eu-peering"
  }
}

resource "aws_vpc_peering_connection_accepter" "eu_accept" {
  provider = aws.eu

  vpc_peering_connection_id = aws_vpc_peering_connection.us_to_eu.id
  auto_accept               = true

  tags = {
    Name = "arbees-us-eu-peering"
  }
}

# Route tables for peering
resource "aws_route" "us_to_eu" {
  provider = aws.us

  count                     = length(module.us_vpc.private_route_table_ids)
  route_table_id            = module.us_vpc.private_route_table_ids[count.index]
  destination_cidr_block    = var.vpc_cidr_eu
  vpc_peering_connection_id = aws_vpc_peering_connection.us_to_eu.id
}

resource "aws_route" "eu_to_us" {
  provider = aws.eu

  count                     = length(module.eu_vpc.private_route_table_ids)
  route_table_id            = module.eu_vpc.private_route_table_ids[count.index]
  destination_cidr_block    = var.vpc_cidr_us
  vpc_peering_connection_id = aws_vpc_peering_connection.us_to_eu.id
}

# ==============================================================================
# Outputs
# ==============================================================================

output "us_api_endpoint" {
  description = "US API ALB endpoint"
  value       = module.us_alb.dns_name
}

output "us_database_endpoint" {
  description = "US TimescaleDB endpoint"
  value       = module.us_database.endpoint
  sensitive   = true
}

output "us_redis_endpoint" {
  description = "US Redis endpoint"
  value       = module.us_redis.endpoint
  sensitive   = true
}

output "eu_polymarket_proxy_endpoint" {
  description = "EU Polymarket proxy internal endpoint"
  value       = "polymarket-proxy.eu-central-1.internal:8080"
}
