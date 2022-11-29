use std::marker::PhantomData;

use crate::util::{is_reducing_barrier_step, is_streaming_source_step};
use crate::ExecutionResult;
use crate::{err::Error, storage::DatastoreRef, IxResult, SimpleTransaction, VertexRepository};
use gremlin::process::traversal::{GraphTraversal, Terminator, TerminatorToken};
use gremlin::GremlinError;
use gremlin::{
	process::traversal::{Bytecode, Instruction},
	FromGValue, GValue, List, Vertex,
};

use super::StepCollector;

#[derive(Clone)]
pub struct StepExecutor<'a, T: FromGValue + Clone> {
	bytecode: Bytecode,
	pub result: ExecutionResult,
	terminator: TerminatorToken,
	source: String,
	v: VertexRepository<'a>,
	phantom: PhantomData<T>,
	iter_index: usize,
}

impl<'a, T: FromGValue + Clone> StepExecutor<'a, T> {
	pub fn new<S, E>(traversal: &GraphTraversal<S, T, E>, ds_ref: DatastoreRef<'a>) -> Self
	where
		T: FromGValue,
		E: Terminator<T>,
	{
		StepExecutor {
			bytecode: traversal.bytecode().clone(),
			result: ExecutionResult::default(),
			terminator: TerminatorToken::Null,
			source: String::default(),
			v: VertexRepository::new(ds_ref),
			phantom: PhantomData,
			iter_index: 0,
		}
	}

	async fn process_streaming_step(&mut self, step: &Instruction) {
		let args = step.args();
		let operator = step.operator().as_str();
		let source = operator.to_string();
		match operator {
			"V" => {
				let result = self.v(args).await;
				self.result.vertices = result;
			}
			"E" => {
				let result = self.e(args).await;
				self.result.edges = result;
			}
			"addV" => {
				let result = self.add_v(args).await;
				self.result.new_vertices = result;
			}
			"addE" => {
				let result = self.add_e(args).await;
				self.result.new_edges = result;
			}
			_ => unimplemented!(),
		};

		self.source = source;
	}

	async fn process_reducing_barrier_step(&mut self, step: &Instruction) {
		let args = step.args();
		let operator = step.operator().as_str();
		let result = match operator {
			"count" => self.count(args).await,
			_ => unimplemented!(),
		};

		self.result.other = result;
	}

	async fn process_step(&mut self, step: &Instruction) {
		let args = step.args();
		let operator = step.operator().as_str();
		let result = match operator {
			"property" => self.property(args).await,
			"properties" => self.properties(args).await,
			"count" => self.count(args).await,
			"hasLabel" => self.has_label(args).await,
			"hasIds" => self.has_id(args).await,
			_ => unimplemented!(),
		};

		self.result.other = result;
	}

	async fn execute(&mut self) -> Result<GValue, GremlinError>
	where
		T: FromGValue,
	{
		#[cfg(feature = "debug-suite")]
		self.bytecode_debug();

		for step in self.bytecode.clone().steps() {
			match step.operator().as_str() {
				s if is_streaming_source_step(s) => self.process_streaming_step(step).await,
				s if is_reducing_barrier_step(s) => self.process_reducing_barrier_step(step).await,
				_ => self.process_step(step).await,
			}
		}
		let collector = StepCollector::new(self.clone());
		let result = collector.collect(&self.terminator.clone()).unwrap();

		#[cfg(feature = "debug-suite")]
		self.collect_debug(result.clone());

		Ok(result)
	}

	pub async fn done(&mut self) -> Result<T, Error>
	where
		T: FromGValue + Clone,
	{
		let exec = self.execute().await.unwrap();
		let value = T::from_gvalue(exec).unwrap();
		Ok(value)
	}

	pub async fn to_list(&mut self) -> Result<Vec<T>, Error>
	where
		T: FromGValue + Clone,
	{
		let mut result = vec![];
		let exec = self.execute().await.unwrap();
		let list = exec.get::<List>().unwrap();
		for item in list.iter() {
			let value = T::from_gvalue(item.clone()).unwrap();
			result.push(value);
		}

		Ok(result)
	}

	pub async fn next(&mut self) -> Result<Option<T>, Error>
	where
		T: FromGValue + Clone,
	{
		let list = self.to_list().await.unwrap();
		Ok(if self.iter_index < list.len() {
			let result = list[self.iter_index].clone();
			let option = Some(result);
			self.iter_index += 1;
			option
		} else {
			None
		})
	}

	pub async fn has_next(&mut self) -> Result<bool, Error>
	where
		T: FromGValue + Clone,
	{
		let list = self.to_list().await.unwrap();
		Ok(self.iter_index + 1 < list.len())
	}

	/// The V()-step is meant to read vertices from the graph and is usually
	/// used to start a GraphTraversal, but can also be used mid-traversal.
	async fn v(&mut self, args: &Vec<GValue>) -> IxResult {
		let tx = &mut self.v.mut_tx();
		let result = self.v.v(tx, args).await.unwrap();

		self.set_terminator(TerminatorToken::Vertex);
		IxResult::new("V", GValue::List(result))
	}

	async fn e(&mut self, _ids: &Vec<GValue>) -> IxResult {
		self.set_terminator(TerminatorToken::Edge);
		IxResult::new("E", GValue::Null)
	}

	/// The addV()-step is used to add vertices to the graph (map/sideEffect).
	/// For every incoming object, a vertex is created. Moreover, GraphTraversalSource maintains an addV() method.
	/// [Documentation](https://tinkerpop.apache.org/docs/current/reference/#addvertex-step)
	async fn add_v(&mut self, args: &Vec<GValue>) -> IxResult {
		let tx = &mut self.v.mut_tx();
		let vertex = self.v.new_v(tx, args).await.unwrap();

		// Push new vertex to the end of vertices
		let mut vertices = self.source_value::<List>("addV").unwrap();
		vertices.push(GValue::Vertex(vertex));
		self.result.new_vertices.value = GValue::List(vertices.clone());

		tx.commit().await.unwrap();
		self.set_terminator(TerminatorToken::Vertex);
		IxResult::new("addV", GValue::List(vertices))
	}

	async fn add_e(&mut self, _labels: &Vec<GValue>) -> IxResult {
		self.set_terminator(TerminatorToken::Edge);
		IxResult::new("addE", GValue::Null)
	}

	async fn property_with_cardinality(&mut self, _args: &Vec<GValue>) -> IxResult {
		IxResult::new("property", GValue::Null)
	}

	async fn vertex_property(&mut self, args: &Vec<GValue>) -> IxResult {
		let tx = &mut self.v.mut_tx();
		let mut result: Vec<GValue> = vec![];
		let source = &self.source.clone();
		let vertices = self.list_from_source::<Vertex>(source, None).unwrap();
		match vertices {
			v if v.is_empty() => {
				let vertex = self.v.new_property(tx, args).await.unwrap();
				result.push(GValue::Vertex(vertex))
			}
			mut v => {
				for cur in v.iter_mut() {
					let vertex = self.v.property(cur, tx, args).await.unwrap();
					result.push(GValue::Vertex(vertex));
				}
			}
		}
		tx.commit().await.unwrap();

		let list = GValue::List(List::new(result));
		IxResult::new("vertex_property", list)
	}

	async fn add_vertex_property(&mut self, args: &Vec<GValue>) -> IxResult {
		let tx = &mut self.v.mut_tx();
		let stream = self.result.get_from_source(&self.source);
		let mut vertices = stream.value.get::<List>().unwrap().clone();

		// Create a new property
		let last = vertices.last_mut().unwrap();
		let vertex = last.get::<Vertex>().unwrap();
		let result = self.v.property(&mut vertex.clone(), tx, args).await.unwrap();

		// Mutate last vertex in vertices
		let value = GValue::Vertex(result);
		*last = value.clone();

		self.result.new_vertices.value = GValue::List(vertices);
		tx.commit().await.unwrap();

		IxResult::new("vertex_property", value)
	}

	async fn vertices_properties(&mut self, args: &Vec<GValue>) -> IxResult {
		let mut result = vec![];
		let source = &self.source.clone();
		let mut vertices = self.list_from_source::<Vertex>(source, None).unwrap();
		if !vertices.is_empty() {
			for cur in vertices.iter_mut() {
				let vertex = self.v.properties(cur, args).await.unwrap();
				result.push(GValue::Vertex(vertex));
			}
			self.result.vertices.value = GValue::List(List::new(result.clone()));
		}
		self.set_terminator(TerminatorToken::VertexProperty);
		let list = GValue::List(List::new(result));
		IxResult::new("properties", list)
	}

	async fn new_vertex_properties(&mut self, args: &Vec<GValue>) -> IxResult {
		let source = &self.source.clone();
		let new_vertices = self.source_value::<List>(source).unwrap();
		let cur = new_vertices.last().unwrap();
		let vertex = cur.get::<Vertex>().unwrap();
		let vertex_with_properties = self.v.properties(&mut vertex.clone(), args).await.unwrap();
		let result = GValue::Vertex(vertex_with_properties);
		self.result.vertices.value = result.clone();

		self.set_terminator(TerminatorToken::VertexProperty);
		IxResult::new("properties", result)
	}

	async fn properties(&mut self, args: &Vec<GValue>) -> IxResult {
		match self.source.as_str() {
			"V" => self.vertices_properties(args).await,
			"addV" => self.new_vertex_properties(args).await,
			_ => unimplemented!(),
		}
	}

	/// The property()-step is used to add properties to the elements of the graph (sideEffect).
	/// Unlike addV() and addE(), property() is a full sideEffect step in that it does not return
	/// the property it created, but the element that streamed into it. Moreover, if property()
	/// follows an addV() or addE(), then it is "folded" into the previous step to enable vertex
	/// and edge creation with all its properties in one creation operation.
	/// [Documentation](https://tinkerpop.apache.org/docs/current/reference/#property-step)
	async fn property(&mut self, args: &Vec<GValue>) -> IxResult {
		match args.first().unwrap().is_cardinality() {
			true => self.property_with_cardinality(args).await,
			false => match self.source.as_str() {
				"V" => self.vertex_property(args).await,
				"addV" => self.add_vertex_property(args).await,
				_ => unimplemented!(),
			},
		}
	}

	async fn count(&mut self, _args: &Vec<GValue>) -> IxResult
	where
		T: FromGValue + Clone,
	{
		let streamed_terminator = GValue::Terminator(self.terminator.clone());
		self.set_terminator(TerminatorToken::Int64);
		IxResult::new("count", streamed_terminator)
	}

	async fn has_label(&mut self, args: &Vec<GValue>) -> IxResult {
		let arg = args.first();
		if arg.is_some() {
			let label = arg.unwrap().get::<String>().unwrap();
			let vertices =
				self.raw_list_from_source::<Vertex>("V", Some(&|v| v.label() == label)).unwrap();
			let list = GValue::List(List::new(vertices));
			self.result.vertices.value = list;

			let new_vertices =
				self.raw_list_from_source::<Vertex>("addV", Some(&|v| v.label() == label)).unwrap();
			let list = GValue::List(List::new(new_vertices));
			self.result.new_vertices.value = list;
		}
		IxResult::new("has_label", GValue::Null)
	}

	async fn has_id(&mut self, _args: &Vec<GValue>) -> IxResult {
		unimplemented!()
	}

	pub fn source_value<E>(&self, source: &str) -> Result<E, GremlinError>
	where
		E: FromGValue,
	{
		let stream = self.result.get_from_source(source);
		let item = E::from_gvalue(stream.value);
		item
	}

	fn raw_list_from_source<E>(
		&mut self,
		source: &str,
		cond: Option<&(dyn Fn(&E) -> bool)>,
	) -> Result<Vec<GValue>, Error>
	where
		E: FromGValue + Clone,
	{
		let stream = self.result.get_from_source(source);
		let list = stream.value.get::<List>().unwrap();
		let mut result = vec![];
		for item in list.iter() {
			let value = E::from_gvalue(item.clone()).unwrap();
			match cond {
				Some(f) => {
					if f(&value) {
						result.push(item.clone());
					}
				}
				None => result.push(item.clone()),
			}
		}
		Ok(result)
	}

	fn list_from_source<E>(
		&mut self,
		source: &str,
		cond: Option<&(dyn Fn(&E) -> bool)>,
	) -> Result<Vec<E>, Error>
	where
		E: FromGValue + Clone,
	{
		let stream = self.result.get_from_source(source);
		let list = stream.value.get::<List>().unwrap();
		let mut result = vec![];
		for item in list.iter() {
			let value = E::from_gvalue(item.clone()).unwrap();
			match cond {
				Some(f) => {
					if f(&value) {
						result.push(value);
					}
				}
				None => result.push(value),
			}
		}
		Ok(result)
	}

	#[cfg(feature = "debug-suite")]
	fn collect_debug(&self, result: GValue) {
		println!("==> Result");
		println!("{:?}", result);
		println!("-----------------");
	}

	#[cfg(feature = "debug-suite")]
	fn bytecode_debug(&self) {
		println!("==> Bytecode");
		for (index, ix) in self.bytecode.steps().iter().enumerate() {
			println!("Instruction {:?}: {:?}", index, ix);
		}
		println!("-----------------");
	}

	fn set_terminator(&mut self, token: TerminatorToken) {
		self.terminator = token;
	}
}
