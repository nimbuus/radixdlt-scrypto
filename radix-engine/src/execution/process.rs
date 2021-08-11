use std::fmt;
use std::time::Instant;

use colored::*;
use sbor::collections::*;
use sbor::*;
use scrypto::buffer::*;
use scrypto::kernel::*;
use scrypto::types::*;
use wasmi::*;

use crate::execution::*;
use crate::ledger::*;
use crate::model::*;

/// A runnable blueprint instance.
pub struct Process<'m, 'rt, 'le, L: Ledger> {
    runtime: &'rt mut Runtime<'le, L>,
    blueprint: Address,
    component: String,
    method: String,
    args: Vec<Vec<u8>>,
    depth: usize,
    module: &'m ModuleRef,
    memory: &'m MemoryRef,
    buckets: HashMap<BID, Bucket>,
    buckets_lent: HashMap<BID, Bucket>,
    buckets_borrowed: HashMap<BID, BucketRef>,
    buckets_moving: HashMap<BID, Bucket>,
}

impl<'m, 'rt, 'le, L: Ledger> Process<'m, 'rt, 'le, L> {
    pub fn new(
        runtime: &'rt mut Runtime<'le, L>,
        blueprint: Address,
        component: String,
        method: String,
        args: Vec<Vec<u8>>,
        depth: usize,
        module: &'m ModuleRef,
        memory: &'m MemoryRef,
        buckets: HashMap<BID, Bucket>,
    ) -> Self {
        Self {
            runtime,
            blueprint,
            component,
            method,
            args,
            depth,
            module,
            memory,
            buckets,
            buckets_lent: HashMap::new(),
            buckets_borrowed: HashMap::new(),
            buckets_moving: HashMap::new(),
        }
    }

    /// Start this process by invoking the component main method.
    pub fn run(&mut self) -> Result<Vec<u8>, RuntimeError> {
        let now = Instant::now();
        self.info(format!(
            "CALL started: blueprint = {:?}, component = {:?}, method = {:?}, args = {:?}",
            self.blueprint, self.component, self.method, self.args
        ));

        let func = format!("{}_{}", self.component, "main");
        let result = self.invoke(func);

        self.info(format!(
            "CALL finished: time elapsed = {} ms, result = {:?}",
            now.elapsed().as_millis(),
            result
        ));
        result
    }

    pub fn invoke(&mut self, func: String) -> Result<Vec<u8>, RuntimeError> {
        let invoke_res = self.module.invoke_export(func.as_str(), &[], self);

        match invoke_res.map_err(|e| RuntimeError::InvokeError(e))? {
            Some(RuntimeValue::I32(ptr)) => {
                let bytes = self.read_bytes(ptr)?;
                self.process_sbor_data(&bytes, Self::remove_transient_reject_persisted)?;
                self.finalize()?;
                Ok(bytes)
            }
            _ => Err(RuntimeError::NoValidBlueprintReturn),
        }
    }

    pub fn publish_blueprint(
        &mut self,
        input: PublishBlueprintInput,
    ) -> Result<PublishBlueprintOutput, RuntimeError> {
        let address = self.runtime.new_blueprint_address(&input.code);

        if self.runtime.get_blueprint(address).is_some() {
            return Err(RuntimeError::BlueprintAlreadyExists(address));
        }
        load_module(&input.code)?;

        self.debug(format!(
            "New blueprint: address = {:?}, code length = {:?}",
            address,
            input.code.len()
        ));
        self.runtime
            .put_blueprint(address, Blueprint::new(input.code));

        Ok(PublishBlueprintOutput { blueprint: address })
    }

    pub fn call_blueprint(
        &mut self,
        input: CallBlueprintInput,
    ) -> Result<CallBlueprintOutput, RuntimeError> {
        // load the code
        let (module, memory) = self
            .runtime
            .load_module(input.blueprint)
            .ok_or(RuntimeError::BlueprintNotFound(input.blueprint))?;

        // remove resources
        for arg in &input.args {
            self.process_sbor_data(arg, Self::remove_transient_reject_persisted)?;
        }
        let buckets_out = self.buckets_moving.clone();
        self.buckets_moving.clear();

        // create a process
        let mut process = Process::new(
            self.runtime,
            input.blueprint,
            input.component,
            input.method,
            input.args,
            self.depth + 1,
            &module,
            &memory,
            buckets_out,
        );

        // run!
        let result = process.run();

        // collect resources
        let buckets_in = process.buckets_moving.clone();
        process.buckets_moving.clear();
        self.buckets.extend(buckets_in);

        Ok(CallBlueprintOutput { rtn: result? })
    }

    pub fn create_component(
        &mut self,
        input: CreateComponentInput,
    ) -> Result<CreateComponentOutput, RuntimeError> {
        let address = self.runtime.new_component_address();

        if self.runtime.get_component(address).is_some() {
            return Err(RuntimeError::ComponentAlreadyExists(address));
        }

        let new_state = self.process_sbor_data(&input.state, Self::transient_to_persist)?;
        self.debug(format!(
            "New component: address = {:?}, name = {:?}, state = {:?}, transformed_state = {:?}",
            address, input.name, input.state, new_state
        ));

        let component = Component::new(self.blueprint, input.name, new_state);
        self.runtime.put_component(address, component);

        Ok(CreateComponentOutput { component: address })
    }

    pub fn get_component_info(
        &mut self,
        input: GetComponentInfoInput,
    ) -> Result<GetComponentInfoOutput, RuntimeError> {
        let result = self
            .runtime
            .get_component(input.component)
            .map(|c| ComponentInfo {
                blueprint: c.blueprint().clone(),
                name: c.name().to_string(),
            });
        Ok(GetComponentInfoOutput { result })
    }

    pub fn get_component_state(
        &mut self,
        input: GetComponentStateInput,
    ) -> Result<GetComponentStateOutput, RuntimeError> {
        let component = self
            .runtime
            .get_component(input.component)
            .ok_or(RuntimeError::ComponentNotFound(input.component))?;

        let state = component.state();

        Ok(GetComponentStateOutput {
            state: state.to_owned(),
        })
    }

    pub fn put_component_state(
        &mut self,
        input: PutComponentStateInput,
    ) -> Result<PutComponentStateOutput, RuntimeError> {
        let new_state = self.process_sbor_data(&input.state, Self::transient_to_persist)?;
        self.trace(format!("Transformed: {:?}", new_state));

        let component = self
            .runtime
            .get_component_mut(input.component)
            .ok_or(RuntimeError::ComponentNotFound(input.component))?;

        component.set_state(new_state);

        Ok(PutComponentStateOutput {})
    }

    pub fn create_resource(
        &mut self,
        input: CreateResourceInput,
    ) -> Result<CreateResourceOutput, RuntimeError> {
        let address = self
            .runtime
            .new_resource_address(self.blueprint, input.info.symbol.as_str());

        if self.runtime.get_resource(address).is_some() {
            return Err(RuntimeError::ResourceAlreadyExists(address));
        } else {
            self.debug(format!("New resource: {:?}", address));

            self.runtime
                .put_resource(address, Resource::new(input.info));
        }
        Ok(CreateResourceOutput { resource: address })
    }

    pub fn get_resource_info(
        &mut self,
        input: GetResourceInfoInput,
    ) -> Result<GetResourceInfoOutput, RuntimeError> {
        Ok(GetResourceInfoOutput {
            result: self
                .runtime
                .get_resource(input.resource)
                .map(|r| r.info().clone()),
        })
    }

    pub fn mint_resource(
        &mut self,
        input: MintResourceInput,
    ) -> Result<MintResourceOutput, RuntimeError> {
        let resource = self
            .runtime
            .get_resource(input.resource)
            .ok_or(RuntimeError::ResourceNotFound(input.resource))?
            .info();

        if resource.minter.is_none() {
            Err(RuntimeError::ImmutableResource)
        } else if resource.minter != Some(self.blueprint) {
            Err(RuntimeError::NotAuthorizedToMint)
        } else {
            // TODO: how to keep track of token supply

            let bucket = Bucket::new(input.amount, input.resource);
            let bid = self.runtime.new_transient_bid();
            self.buckets.insert(bid, bucket);
            Ok(MintResourceOutput { bucket: bid })
        }
    }

    pub fn combine_buckets(
        &mut self,
        input: CombineBucketsInput,
    ) -> Result<CombineBucketsOutput, RuntimeError> {
        // The other bucket needs to be a transient bucket
        let other = self
            .buckets
            .remove(&input.other)
            .ok_or(RuntimeError::BucketNotFound)?;

        let bucket = if input.bucket.is_persisted() {
            self.runtime
                .get_bucket_mut(input.bucket)
                .ok_or(RuntimeError::BucketNotFound)?
        } else {
            self.buckets
                .get_mut(&input.bucket)
                .ok_or(RuntimeError::BucketNotFound)?
        };

        bucket
            .put(other)
            .map_err(|e| RuntimeError::AccountingError(e))?;

        Ok(CombineBucketsOutput {})
    }

    pub fn split_bucket(
        &mut self,
        input: SplitBucketInput,
    ) -> Result<SplitBucketOutput, RuntimeError> {
        let bucket = if input.bucket.is_persisted() {
            self.runtime
                .get_bucket_mut(input.bucket)
                .ok_or(RuntimeError::BucketNotFound)?
        } else {
            self.buckets
                .get_mut(&input.bucket)
                .ok_or(RuntimeError::BucketNotFound)?
        };

        let new_bucket = bucket
            .take(input.amount)
            .map_err(|e| RuntimeError::AccountingError(e))?;
        let new_bid = self.runtime.new_transient_bid();
        self.buckets.insert(new_bid, new_bucket);

        Ok(SplitBucketOutput { bucket: new_bid })
    }

    pub fn borrow_bucket(
        &mut self,
        input: BorrowBucketInput,
    ) -> Result<BorrowBucketOutput, RuntimeError> {
        // TODO: how to borrow persisted bucket?

        let bid = input.bucket;
        self.debug(format!("Borrowing {:?}", bid));

        match self.buckets_lent.get_mut(&bid) {
            Some(bucket) => {
                // re-borrow
                self.buckets_borrowed
                    .entry(bid)
                    .or_insert(BucketRef::new(bucket.clone(), 0))
                    .increase_count();
            }
            None => {
                // first time borrow
                let bucket = self
                    .buckets
                    .remove(&bid)
                    .ok_or(RuntimeError::BucketNotFound)?;
                self.buckets_borrowed
                    .insert(bid, BucketRef::new(bucket.clone(), 1));
                self.buckets_lent.insert(bid, bucket);
            }
        }

        Ok(BorrowBucketOutput { reference: bid })
    }

    pub fn return_bucket(
        &mut self,
        input: ReturnBucketInput,
    ) -> Result<ReturnBucketOutput, RuntimeError> {
        let bid = input.reference;
        self.debug(format!("Returning: {:?}", bid));

        let bucket = self
            .buckets_borrowed
            .get_mut(&bid)
            .ok_or(RuntimeError::BucketRefNotFound)?;

        let new_count = bucket
            .decrease_count()
            .map_err(|e| RuntimeError::AccountingError(e))?;
        if new_count == 0 {
            self.buckets_borrowed.remove(&bid);

            if let Some(b) = self.buckets_lent.remove(&bid) {
                self.buckets.insert(bid, b);
            }
        }

        Ok(ReturnBucketOutput {})
    }

    pub fn get_bucket_amount(
        &mut self,
        input: GetBucketAmountInput,
    ) -> Result<GetBucketAmountOutput, RuntimeError> {
        let bucket = self
            .buckets
            .get(&input.bucket)
            .or(self.buckets_lent.get(&input.bucket))
            .or(self
                .buckets_borrowed
                .get(&input.bucket)
                .map(BucketRef::bucket))
            .ok_or(RuntimeError::BucketNotFound)?;

        Ok(GetBucketAmountOutput {
            amount: bucket.amount(),
        })
    }

    pub fn get_bucket_resource(
        &mut self,
        input: GetBucketResourceInput,
    ) -> Result<GetBucketResourceOutput, RuntimeError> {
        let bucket = self
            .buckets
            .get(&input.bucket)
            .or(self.buckets_lent.get(&input.bucket))
            .or(self
                .buckets_borrowed
                .get(&input.bucket)
                .map(BucketRef::bucket))
            .ok_or(RuntimeError::BucketNotFound)?;

        Ok(GetBucketResourceOutput {
            resource: bucket.resource(),
        })
    }

    pub fn withdraw(&mut self, input: WithdrawInput) -> Result<WithdrawOutput, RuntimeError> {
        if input.account != self.blueprint {
            return Err(RuntimeError::UnauthorizedToWithdraw);
        }

        // find the account
        if self.runtime.get_account(input.account).is_none() {
            self.runtime.put_account(input.account, Account::new());
        };
        let account = self.runtime.get_account(input.account).unwrap();

        // look up the bucket
        let bid = match account.get_bucket(input.resource) {
            Some(bid) => *bid,
            None => {
                let bid = self.runtime.new_persisted_bid();
                self.runtime
                    .put_bucket(bid, Bucket::new(U256::zero(), input.resource));

                let acc = self.runtime.get_account_mut(input.account).unwrap();
                acc.insert_bucket(input.resource, bid);

                bid
            }
        };
        let bucket = self
            .runtime
            .get_bucket_mut(bid)
            .expect("The bucket should exist");

        let new_bucket = bucket
            .take(input.amount)
            .map_err(|e| RuntimeError::AccountingError(e))?;
        let new_bid = self.runtime.new_transient_bid();
        self.buckets.insert(new_bid, new_bucket);

        Ok(WithdrawOutput { bucket: new_bid })
    }

    pub fn deposit(&mut self, input: DepositInput) -> Result<DepositOutput, RuntimeError> {
        let to_deposit = self
            .buckets
            .remove(&input.bucket)
            .ok_or(RuntimeError::BucketNotFound)?;

        // find the account
        if self.runtime.get_account(input.account).is_none() {
            self.runtime.put_account(input.account, Account::new());
        };
        let account = self.runtime.get_account(input.account).unwrap();

        // look up the bucket
        let bid = match account.get_bucket(to_deposit.resource()) {
            Some(bid) => *bid,
            None => {
                let bid = self.runtime.new_persisted_bid();
                self.runtime
                    .put_bucket(bid, Bucket::new(U256::zero(), to_deposit.resource()));

                let acc = self.runtime.get_account_mut(input.account).unwrap();
                acc.insert_bucket(to_deposit.resource(), bid);

                bid
            }
        };
        let bucket = self
            .runtime
            .get_bucket_mut(bid)
            .expect("The bucket should exist");

        bucket
            .put(to_deposit)
            .map_err(|e| RuntimeError::AccountingError(e))?;

        Ok(DepositOutput {})
    }

    pub fn emit_log(&mut self, input: EmitLogInput) -> Result<EmitLogOutput, RuntimeError> {
        self.runtime.log(input.level, input.message);

        Ok(EmitLogOutput {})
    }

    pub fn get_context_address(
        &mut self,
        _input: GetContextAddressInput,
    ) -> Result<GetContextAddressOutput, RuntimeError> {
        Ok(GetContextAddressOutput {
            address: self.blueprint,
        })
    }

    pub fn get_call_data(
        &mut self,
        _input: GetCallDataInput,
    ) -> Result<GetCallDataOutput, RuntimeError> {
        Ok(GetCallDataOutput {
            method: self.method.clone(),
            args: self.args.clone(),
        })
    }

    fn process_sbor_data(
        &mut self,
        state: &Vec<u8>,
        transform: fn(&mut Self, BID) -> Result<BID, RuntimeError>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut decoder = Decoder::with_metadata(state);
        let mut encoder = Encoder::with_metadata();

        self.traverse_sbor(None, &mut decoder, &mut encoder, transform)?;

        if decoder.remaining() > 0 {
            // We expect a single SBOR value
            Err(RuntimeError::InvalidData(DecodeError::NotAllBytesUsed(
                decoder.remaining(),
            )))
        } else {
            Ok(encoder.into())
        }
    }

    // TODO: stack overflow
    fn traverse_sbor(
        &mut self,
        ty_from_ctx: Option<u8>,
        dec: &mut Decoder,
        enc: &mut Encoder,
        transform: fn(&mut Self, BID) -> Result<BID, RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let ty = ty_from_ctx.unwrap_or(dec.read_type().map_err(RuntimeError::invalid_data)?);

        match ty {
            constants::TYPE_UNIT => self.dte::<()>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_BOOL => self.dte::<bool>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_I8 => self.dte::<i8>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_I16 => self.dte::<i16>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_I32 => self.dte::<i32>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_I64 => self.dte::<i64>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_I128 => self.dte::<i128>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_U8 => self.dte::<u8>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_U16 => self.dte::<u16>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_U32 => self.dte::<u32>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_U64 => self.dte::<u64>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_U128 => self.dte::<u128>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_STRING => self.dte::<String>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_OPTION => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                // index
                let index = dec.read_index().map_err(RuntimeError::invalid_data)?;
                enc.write_index(index as usize);
                // optional value
                match index {
                    0 => Ok(()),
                    1 => self.traverse_sbor(None, dec, enc, transform),
                    _ => Err(RuntimeError::invalid_data(DecodeError::InvalidIndex(index))),
                }
            }
            constants::TYPE_BOX => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                // value
                self.traverse_sbor(None, dec, enc, transform)
            }
            constants::TYPE_ARRAY => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                // element type
                let ele_ty = dec.read_type().map_err(RuntimeError::invalid_data)?;
                enc.write_type(ele_ty);
                // length
                let len = dec.read_len().map_err(RuntimeError::invalid_data)?;
                enc.write_len(len);
                // values
                for _ in 0..len {
                    self.traverse_sbor(Some(ele_ty), dec, enc, transform)?;
                }
                Ok(())
            }
            constants::TYPE_TUPLE => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                //length
                let len = dec.read_len().map_err(RuntimeError::invalid_data)?;
                enc.write_len(len);
                // values
                for _ in 0..len {
                    self.traverse_sbor(None, dec, enc, transform)?;
                }
                Ok(())
            }
            constants::TYPE_STRUCT => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                // fields
                self.traverse_sbor(None, dec, enc, transform)
            }
            constants::TYPE_ENUM => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                // index
                let index = dec.read_index().map_err(RuntimeError::invalid_data)?;
                enc.write_index(index as usize);
                // name
                let name = dec.read_name().map_err(RuntimeError::invalid_data)?;
                enc.write_name(name.as_str());
                // fields
                self.traverse_sbor(None, dec, enc, transform)
            }
            constants::TYPE_FIELDS_NAMED => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                //length
                let len = dec.read_len().map_err(RuntimeError::invalid_data)?;
                enc.write_len(len);
                // named fields
                for _ in 0..len {
                    // name
                    let name = dec.read_name().map_err(RuntimeError::invalid_data)?;
                    enc.write_name(name.as_str());
                    // value
                    self.traverse_sbor(None, dec, enc, transform)?;
                }
                Ok(())
            }
            constants::TYPE_FIELDS_UNNAMED => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                //length
                let len = dec.read_len().map_err(RuntimeError::invalid_data)?;
                enc.write_len(len);
                // named fields
                for _ in 0..len {
                    // value
                    self.traverse_sbor(None, dec, enc, transform)?;
                }
                Ok(())
            }
            constants::TYPE_FIELDS_UNIT => {
                if ty_from_ctx.is_none() {
                    enc.write_type(ty);
                }
                Ok(())
            }
            // collections
            constants::TYPE_VEC => {
                todo!()
            }
            constants::TYPE_TREE_SET | constants::TYPE_HASH_SET => {
                todo!()
            }
            constants::TYPE_TREE_MAP | constants::TYPE_HASH_MAP => {
                todo!()
            }
            // scrypto types
            constants::TYPE_H256 => self.dte::<H256>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_U256 => self.dte::<U256>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_ADDRESS => self.dte::<Address>(ty_from_ctx, ty, dec, enc, |_, v| Ok(v)),
            constants::TYPE_BID => self.dte::<BID>(ty_from_ctx, ty, dec, enc, transform),
            _ => Err(RuntimeError::InvalidData(DecodeError::InvalidType {
                expected: 0xff,
                actual: ty,
            })),
        }
    }

    /// Decode, transform and encode
    fn dte<T: Decode + Encode + std::fmt::Debug>(
        &mut self,
        ty_from_ctx: Option<u8>,
        ty: u8,
        dec: &mut Decoder,
        enc: &mut Encoder,
        transform: fn(&mut Self, T) -> Result<T, RuntimeError>,
    ) -> Result<(), RuntimeError> {
        if ty_from_ctx.is_none() {
            enc.write_type(ty);
        }

        transform(
            self,
            T::decode_value(dec).map_err(RuntimeError::invalid_data)?,
        )?
        .encode_value(enc);

        Ok(())
    }

    /// Convert transient buckets to persisted buckets
    fn transient_to_persist(&mut self, bid: BID) -> Result<BID, RuntimeError> {
        if bid.is_transient() {
            let bucket = self
                .buckets
                .remove(&bid)
                .ok_or(RuntimeError::BucketNotFound)?;
            let new_bid = self.runtime.new_persisted_bid();
            self.runtime.put_bucket(new_bid, bucket);
            self.debug(format!("Converting {:?} to {:?}", bid, new_bid));
            Ok(new_bid)
        } else {
            Ok(bid)
        }
    }

    /// Remove transient buckets from this process, and reject persisted buckets.
    fn remove_transient_reject_persisted(&mut self, bid: BID) -> Result<BID, RuntimeError> {
        if bid.is_transient() {
            let bucket = self
                .buckets
                .remove(&bid)
                .ok_or(RuntimeError::BucketNotFound)?;
            self.buckets_moving.insert(bid, bucket);
            Ok(bid)
        } else {
            Err(RuntimeError::PersistedBucketCantBeMoved)
        }
    }

    /// Finalize this process.
    fn finalize(&self) -> Result<(), RuntimeError> {
        let mut buckets = vec![];

        for (bid, bucket) in &self.buckets {
            if bucket.amount() != U256::zero() {
                self.error(format!("Burning bucket: {:?} {:?}", bid, bucket));
                buckets.push(*bid);
            }
        }
        for (bid, bucket) in &self.buckets_lent {
            self.error(format!("Bucket lent: {:?} {:?}", bid, bucket));
            buckets.push(*bid);
        }

        for (bid, bucket_ref) in &self.buckets_borrowed {
            self.error(format!("Bucket lent: {:?} {:?}", bid, bucket_ref));
            buckets.push(*bid);
        }

        if buckets.is_empty() {
            Ok(())
        } else {
            Err(RuntimeError::ResourceLeak(buckets))
        }
    }

    /// Send a byte array to this process.
    fn send_bytes(&mut self, bytes: &[u8]) -> Result<i32, RuntimeError> {
        let result = self.module.invoke_export(
            "scrypto_alloc",
            &[RuntimeValue::I32((bytes.len()) as i32)],
            &mut NopExternals,
        );

        match result {
            Ok(Some(RuntimeValue::I32(ptr))) => {
                if self.memory.set(ptr as u32, bytes).is_ok() {
                    return Ok(ptr);
                }
            }
            _ => {}
        }

        Err(RuntimeError::UnableToAllocateMemory)
    }

    /// Read a length-prefixed byte array from this process.
    fn read_bytes(&mut self, ptr: i32) -> Result<Vec<u8>, RuntimeError> {
        let a = self
            .memory
            .get((ptr - 4) as u32, 4)
            .map_err(|e| RuntimeError::MemoryAccessError(e))?;
        let len = u32::from_le_bytes([a[0], a[1], a[2], a[3]]);

        self.memory
            .get(ptr as u32, len as usize)
            .map_err(|e| RuntimeError::MemoryAccessError(e))
    }

    /// Log a message to console.
    fn log(&self, level: Level, msg: String) {
        let (l, m) = match level {
            Level::Error => ("ERROR".red(), msg.to_string().red()),
            Level::Warn => ("WARN".yellow(), msg.to_string().yellow()),
            Level::Info => ("INFO".green(), msg.to_string().green()),
            Level::Debug => ("DEBUG".cyan(), msg.to_string().cyan()),
            Level::Trace => ("TRACE".normal(), msg.to_string().normal()),
        };

        println!("{}[{:5}] {}", "  ".repeat(self.depth), l, m);
    }

    fn error<T: ToString>(&self, msg: T) {
        self.log(Level::Error, msg.to_string());
    }

    #[allow(dead_code)]
    fn warn<T: ToString>(&self, msg: T) {
        self.log(Level::Warn, msg.to_string());
    }

    fn info<T: ToString>(&self, msg: T) {
        self.log(Level::Info, msg.to_string());
    }

    fn trace<T: ToString>(&self, msg: T) {
        self.log(Level::Trace, msg.to_string());
    }

    fn debug<T: ToString>(&self, msg: T) {
        self.log(Level::Debug, msg.to_string());
    }

    /// Handle a kernel call.
    fn handle<I: Decode + fmt::Debug, O: Encode + fmt::Debug>(
        &mut self,
        args: RuntimeArgs,
        handler: fn(&mut Self, input: I) -> Result<O, RuntimeError>,
        trace: bool,
    ) -> Result<Option<RuntimeValue>, Trap> {
        let now = Instant::now();
        let input_ptr: u32 = args.nth_checked(1)?;
        let input_len: u32 = args.nth_checked(2)?;
        let input_bytes = self
            .memory
            .get(input_ptr, input_len as usize)
            .map_err(|e| Trap::from(RuntimeError::MemoryAccessError(e)))?;
        let input: I = scrypto_decode(&input_bytes)
            .map_err(|e| Trap::from(RuntimeError::InvalidRequest(e)))?;
        if trace {
            self.trace(format!("input = {:?}", input));
        }

        let output: O = handler(self, input).map_err(Trap::from)?;
        let output_bytes = scrypto_encode(&output);
        let output_ptr = self.send_bytes(&output_bytes).map_err(Trap::from)?;
        if trace {
            self.trace(format!(
                "output = {:?}, time = {} ms",
                output,
                now.elapsed().as_millis()
            ));
        }

        Ok(Some(RuntimeValue::I32(output_ptr)))
    }
}

impl<'m, 'rt, 'le, T: Ledger> Externals for Process<'m, 'rt, 'le, T> {
    fn invoke_index(
        &mut self,
        index: usize,
        args: RuntimeArgs,
    ) -> Result<Option<RuntimeValue>, Trap> {
        match index {
            KERNEL => {
                let operation: u32 = args.nth_checked(0)?;
                match operation {
                    PUBLISH_BLUEPRINT => self.handle(args, Process::publish_blueprint, false),
                    CALL_BLUEPRINT => self.handle(args, Process::call_blueprint, true),

                    CREATE_COMPONENT => self.handle(args, Process::create_component, true),
                    GET_COMPONENT_INFO => self.handle(args, Process::get_component_info, true),
                    GET_COMPONENT_STATE => self.handle(args, Process::get_component_state, true),
                    PUT_COMPONENT_STATE => self.handle(args, Process::put_component_state, true),

                    CREATE_RESOURCE => self.handle(args, Process::create_resource, true),
                    GET_RESOURCE_INFO => self.handle(args, Process::get_resource_info, true),
                    MINT_RESOURCE => self.handle(args, Process::mint_resource, true),

                    COMBINE_BUCKETS => self.handle(args, Process::combine_buckets, true),
                    SPLIT_BUCKET => self.handle(args, Process::split_bucket, true),
                    BORROW_BUCKET => self.handle(args, Process::borrow_bucket, true),
                    RETURN_BUCKET => self.handle(args, Process::return_bucket, true),
                    GET_BUCKET_AMOUNT => self.handle(args, Process::get_bucket_amount, true),
                    GET_BUCKET_RESOURCE => self.handle(args, Process::get_bucket_resource, true),

                    WITHDRAW => self.handle(args, Process::withdraw, true),
                    DEPOSIT => self.handle(args, Process::deposit, true),

                    EMIT_LOG => self.handle(args, Process::emit_log, true),
                    GET_CONTEXT_ADDRESS => self.handle(args, Process::get_context_address, true),
                    GET_CALL_DATA => self.handle(args, Process::get_call_data, true),
                    _ => Err(RuntimeError::InvalidOpCode(operation).into()),
                }
            }
            _ => Err(RuntimeError::UnknownHostFunction(index).into()),
        }
    }
}
