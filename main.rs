use std::collections::HashMap;
use std::error::Error;
use std::io::{self, Write};
use serde::{Deserialize, Serialize};
use stellar_wallet::Stellar;

// ============================================================================
// ENUMS & STRUCTS
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy)]
enum StrategyType {
    AquaLiquidityPool,
    YieldBloxLending,
    MoneyMarket,
}

#[derive(Debug, Clone)]
struct Strategy {
    strategy_type: StrategyType,
    allocation_percentage: u8,
    current_apy: u16,
    total_allocated: u64,
    current_yield: u64,
}

#[derive(Debug)]
struct Vault {
    risk_level: RiskLevel,
    total_value: u64,
    total_shares: u64,
    insurance_fee: u16,
    strategies: Vec<Strategy>,
}

impl Vault {
    fn get_share_price(&self) -> u64 {
        if self.total_shares == 0 {
            10_000_000
        } else {
            (self.total_value as u128 * 10_000_000 / self.total_shares as u128) as u64
        }
    }
}

#[derive(Debug, Clone)]
struct UserPosition {
    shares: u64,
    accumulated_yield: u64,
}

// ============================================================================
// STELLAR INTEGRATION
// ============================================================================

struct StellarClient {
    secret_key: String,
    public_key: String,
    stellar: Stellar,
}

impl StellarClient {
    fn new(secret_key: &str, public_key: &str) -> Result<Self, Box<dyn Error>> {
        if !secret_key.starts_with('S') || secret_key.len() != 56 {
            return Err("Invalid Stellar secret key format (must start with S and be 56 chars)".into());
        }
        
        if !public_key.starts_with('G') || public_key.len() != 56 {
            return Err("Invalid Stellar public key format (must start with G and be 56 chars)".into());
        }
        
        let stellar = Stellar::new("https://horizon-testnet.stellar.org");
        
        Ok(StellarClient {
            secret_key: secret_key.to_string(),
            public_key: public_key.to_string(),
            stellar,
        })
    }

    fn get_public_key(&self) -> String {
        self.public_key.clone()
    }

    async fn get_balance(&self) -> Result<f64, Box<dyn Error>> {
        match self.stellar.get_balance(&self.public_key).await {
            Ok(balances) => {
                // stellar_wallet returns Vec<serde_json::Value>
                // We need to extract the XLM balance from the first element
                if let Some(balance_obj) = balances.get(0) {
                    if let Some(balance_str) = balance_obj.get("balance") {
                        let balance: f64 = balance_str.as_str()
                            .unwrap_or("0")
                            .parse()
                            .unwrap_or(0.0);
                        return Ok(balance);
                    }
                }
                Ok(0.0)
            }
            Err(e) => Err(format!("Failed to get balance: {}", e).into())
        }
    }

    async fn send_payment(&self, destination: &str, amount_xlm: &str) -> Result<String, Box<dyn Error>> {
        println!("\n🚀 Submitting transaction to Stellar Testnet...");
        println!("   From (USER): {}", self.public_key);
        println!("   To (VAULT): {}", destination);
        println!("   Amount: {} XLM", amount_xlm);
        println!("   Using secret key starting with: {}...", &self.secret_key[..5]);
        
        match self.stellar.transfer_xlm(&self.secret_key, destination, amount_xlm).await {
            Ok(_) => {
                println!("\n✅ TRANSACTION SUCCESSFUL!");
                println!("   🔗 View on StellarScan:");
                println!("      Your Account: https://testnet.stellarscan.io/account/{}", self.public_key);
                println!("      Vault Account: https://testnet.stellarscan.io/account/{}", destination);
                Ok("Transaction completed successfully".to_string())
            }
            Err(e) => {
                Err(format!("Transaction failed: {}", e).into())
            }
        }
    }
}

// ============================================================================
// STELLARVAULT
// ============================================================================

struct StellarVault {
    vaults: HashMap<RiskLevel, Vault>,
    user_positions: HashMap<(String, RiskLevel), UserPosition>,
    insurance_pool: u64,
    stellar_client: StellarClient,
    vault_address: String,
}

impl StellarVault {
    fn new(user_secret_key: &str, user_public_key: &str, vault_address: &str) -> Result<Self, Box<dyn Error>> {
        let mut vaults = HashMap::new();
        
        vaults.insert(RiskLevel::Low, Vault {
            risk_level: RiskLevel::Low,
            total_value: 0,
            total_shares: 0,
            insurance_fee: 50,
            strategies: vec![
                Strategy {
                    strategy_type: StrategyType::YieldBloxLending,
                    allocation_percentage: 100,
                    current_apy: 350,
                    total_allocated: 0,
                    current_yield: 0,
                },
            ],
        });

        vaults.insert(RiskLevel::Medium, Vault {
            risk_level: RiskLevel::Medium,
            total_value: 0,
            total_shares: 0,
            insurance_fee: 100,
            strategies: vec![
                Strategy {
                    strategy_type: StrategyType::AquaLiquidityPool,
                    allocation_percentage: 60,
                    current_apy: 850,
                    total_allocated: 0,
                    current_yield: 0,
                },
                Strategy {
                    strategy_type: StrategyType::YieldBloxLending,
                    allocation_percentage: 40,
                    current_apy: 400,
                    total_allocated: 0,
                    current_yield: 0,
                },
            ],
        });

        vaults.insert(RiskLevel::High, Vault {
            risk_level: RiskLevel::High,
            total_value: 0,
            total_shares: 0,
            insurance_fee: 200,
            strategies: vec![
                Strategy {
                    strategy_type: StrategyType::MoneyMarket,
                    allocation_percentage: 100,
                    current_apy: 1500,
                    total_allocated: 0,
                    current_yield: 0,
                },
            ],
        });

        let client = StellarClient::new(user_secret_key, user_public_key)?;
        
        Ok(StellarVault {
            vaults,
            user_positions: HashMap::new(),
            insurance_pool: 0,
            stellar_client: client,
            vault_address: vault_address.to_string(),
        })
    }

    async fn deposit(&mut self, user: &str, risk: RiskLevel, amount_stroops: u64) -> Result<u64, Box<dyn Error>> {
        let amount_xlm = amount_stroops as f64 / 10_000_000.0;
        let amount_xlm_str = format!("{}", amount_xlm);
        
        println!("\n💼 Initiating deposit to StellarVault (SYIA)...");
        println!("   Risk Level: {:?}", risk);
        println!("   Amount: {} XLM", amount_xlm);
        
        // Check user's balance before transaction
        match self.stellar_client.get_balance().await {
            Ok(balance) => {
                println!("\n💰 Account Balance:");
                println!("   Current: {:.2} XLM", balance);
                println!("   After Deposit: {:.2} XLM", balance - amount_xlm);
                
                if balance < amount_xlm + 1.0 {
                    return Err("Insufficient balance for this transaction".into());
                }
            }
            Err(e) => {
                println!("   ⚠️  Could not fetch account info: {}", e);
            }
        }
        
        // Send the payment
        match self.stellar_client.send_payment(&self.vault_address, &amount_xlm_str).await {
            Ok(_) => {
                println!("\n🎉 Transaction submitted to Stellar Network!");
            }
            Err(e) => {
                return Err(format!("Transaction failed: {}", e).into());
            }
        }

        let vault = self.vaults.get_mut(&risk).ok_or("Vault not found")?;
        let share_price = vault.get_share_price();
        let shares_to_mint = (amount_stroops as u128 * 10_000_000 / share_price as u128) as u64;

        let insurance_amount = (amount_stroops as u128 * vault.insurance_fee as u128 / 10000) as u64;
        let net_deposit = amount_stroops - insurance_amount;

        self.insurance_pool += insurance_amount;
        vault.total_value += net_deposit;
        vault.total_shares += shares_to_mint;

        for strategy in &mut vault.strategies {
            let alloc = (net_deposit as u128 * strategy.allocation_percentage as u128 / 100) as u64;
            strategy.total_allocated += alloc;
        }

        let key = (user.to_string(), risk);
        self.user_positions.entry(key)
            .or_insert(UserPosition { shares: 0, accumulated_yield: 0 })
            .shares += shares_to_mint;

        Ok(shares_to_mint)
    }

    fn get_vault_info(&self, risk: RiskLevel) -> Option<&Vault> {
        self.vaults.get(&risk)
    }
}

fn risk_level_to_string(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Low => "Low",
        RiskLevel::Medium => "Medium",
        RiskLevel::High => "High",
    }
}

fn get_user_input(prompt: &str) -> String {
    print!("{}", prompt);
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().to_string()
}

// ============================================================================
// MAIN FUNCTION
// ============================================================================

#[tokio::main]
async fn main() {
    println!("🌟 StellarVault (SYIA) - Smart Yield Insurance Aggregator 🌟\n");
    
    // YOUR ACTUAL ACCOUNTS
    let user_secret_key = "SCT3AR46YPEOBWSRIRD7I74BVFI2PNQULEZB4QAG7XJFU3JBMTS53ZHT";
    let user_public_key = "GCBVQ4OOQY2MREIAQMNNBV2ENSBCPN5SKXIOTO4SV3ENVEVYM5XLTYQY";
    let vault_address = "GCZEAWUJY3BRHCOKU6C5WRLCF5RFSGY22UGBPBXWL4T4G4SSEQMIYMCX";
    
    println!("🔐 Connecting to Stellar Testnet...");
    let mut vault = match StellarVault::new(user_secret_key, user_public_key, vault_address) {
        Ok(v) => {
            println!("✅ Connected!");
            println!("👤 Your Address: {}", user_public_key);
            println!("🏦 SYIA Vault Address: {}", vault_address);
            
            // Fetch and display live balance
            match v.stellar_client.get_balance().await {
                Ok(balance) => {
                    println!("💰 Your Live Balance: {:.2} XLM", balance);
                }
                Err(e) => {
                    println!("⚠️  Could not fetch balance: {}", e);
                }
            }
            
            println!("\n🔗 StellarScan Links:");
            println!("   Your Account: https://testnet.stellarscan.io/account/{}", user_public_key);
            println!("   SYIA Vault: https://testnet.stellarscan.io/account/{}\n", vault_address);
            v
        }
        Err(e) => {
            println!("❌ Failed to connect: {}", e);
            return;
        }
    };

    println!("{}", "=".repeat(70));
    println!("\n📊 StellarVault (SYIA) Risk Levels:\n");
    
    println!("1. 🟢 LOW RISK");
    println!("   - APY: 3.50%");
    println!("   - Insurance Fee: 0.50%");
    println!("   - Strategy: YieldBlox Lending");
    println!("   - Best for: Conservative investors\n");
    
    println!("2. 🟡 MEDIUM RISK");
    println!("   - APY: 8.50%");
    println!("   - Insurance Fee: 1.00%");
    println!("   - Strategy: 60% Aqua LP + 40% YieldBlox");
    println!("   - Best for: Balanced investors\n");
    
    println!("3. 🔴 HIGH RISK");
    println!("   - APY: 15.00%");
    println!("   - Insurance Fee: 2.00%");
    println!("   - Strategy: Money Market");
    println!("   - Best for: Aggressive investors\n");

    println!("{}", "=".repeat(70));

    // Ask user for risk level
    println!("\n💼 Choose your investment strategy:");
    let risk_choice = get_user_input("Enter risk level (low/medium/high): ").to_lowercase();
    
    let risk_level = match risk_choice.as_str() {
        "low" | "l" | "1" => RiskLevel::Low,
        "medium" | "m" | "2" => RiskLevel::Medium,
        "high" | "h" | "3" => RiskLevel::High,
        _ => {
            println!("❌ Invalid choice. Defaulting to Low Risk.");
            RiskLevel::Low
        }
    };

    println!("✅ Selected: {:?} Risk Vault", risk_level);

    // Ask user for deposit amount
    let amount_input = get_user_input("\n💰 Enter deposit amount (XLM): ");
    let amount_xlm: f64 = match amount_input.parse() {
        Ok(amt) if amt > 0.0 => amt,
        _ => {
            println!("❌ Invalid amount. Using default 100 XLM.");
            100.0
        }
    };

    let amount_stroops = (amount_xlm * 10_000_000.0) as u64;

    println!("\n{}", "=".repeat(70));

    // Process deposit
    println!("\n📥 Processing your deposit to SYIA Vault...");
    
    match vault.deposit(user_public_key, risk_level, amount_stroops).await {
        Ok(shares) => {
            let insurance_fee = match risk_level {
                RiskLevel::Low => 0.50,
                RiskLevel::Medium => 1.00,
                RiskLevel::High => 2.00,
            };
            
            println!("\n✅ DEPOSIT COMPLETE!");
            println!("   Amount: {} XLM", amount_xlm);
            println!("   Vault: {:?} Risk", risk_level);
            println!("   Shares Received: {}", shares);
            println!("   Insurance Fee: {:.2}% ({:.2} XLM)", 
                insurance_fee, 
                amount_xlm * insurance_fee / 100.0);
            println!("   Net Investment: {:.2} XLM", 
                amount_xlm * (1.0 - insurance_fee / 100.0));
        },
        Err(e) => println!("❌ Deposit failed: {}", e),
    }

    println!("\n{}", "=".repeat(70));
    println!("\n✅ Transaction complete!");
    println!("\n🔍 Check your transaction on StellarScan:");
    println!("   Your Account: https://testnet.stellarscan.io/account/{}", user_public_key);
    println!("   SYIA Vault: https://testnet.stellarscan.io/account/{}", vault_address);
    println!("\n💡 Refresh StellarScan in a few seconds to see the transaction appear!");
}