#![expect(deprecated)]

use crate::{
    DrawMesh2d, Mesh2d, Mesh2dPipeline, Mesh2dPipelineKey, RenderMesh2dInstances,
    SetMesh2dBindGroup, SetMesh2dViewBindGroup,
};
use bevy_app::{App, Plugin};
use bevy_asset::{Asset, AssetApp, AssetId, AssetServer, Handle};
use bevy_core_pipeline::{
    core_2d::{AlphaMask2d, AlphaMask2dBinKey, Opaque2d, Opaque2dBinKey, Transparent2d},
    tonemapping::{DebandDither, Tonemapping},
};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    prelude::*,
    system::{lifetimeless::SRes, SystemParamItem},
};
use bevy_math::FloatOrd;
use bevy_reflect::{prelude::ReflectDefault, Reflect};
use bevy_render::sync_world::MainEntityHashMap;
use bevy_render::view::RenderVisibleEntities;
use bevy_render::{
    mesh::{MeshVertexBufferLayoutRef, RenderMesh},
    render_asset::{
        prepare_assets, PrepareAssetError, RenderAsset, RenderAssetPlugin, RenderAssets,
    },
    render_phase::{
        AddRenderCommand, BinnedRenderPhaseType, DrawFunctions, PhaseItem, PhaseItemExtraIndex,
        RenderCommand, RenderCommandResult, SetItemPipeline, TrackedRenderPass,
        ViewBinnedRenderPhases, ViewSortedRenderPhases,
    },
    render_resource::{
        AsBindGroup, AsBindGroupError, BindGroup, BindGroupId, BindGroupLayout,
        OwnedBindingResource, PipelineCache, RenderPipelineDescriptor, Shader, ShaderRef,
        SpecializedMeshPipeline, SpecializedMeshPipelineError, SpecializedMeshPipelines,
    },
    renderer::RenderDevice,
    view::{ExtractedView, InheritedVisibility, Msaa, ViewVisibility, Visibility},
    Extract, ExtractSchedule, Render, RenderApp, RenderSet,
};
use bevy_transform::components::{GlobalTransform, Transform};
use bevy_utils::tracing::error;
use core::{hash::Hash, marker::PhantomData};
use derive_more::derive::From;

/// Materials are used alongside [`Material2dPlugin`], [`Mesh2d`], and [`MeshMaterial2d`]
/// to spawn entities that are rendered with a specific [`Material2d`] type. They serve as an easy to use high level
/// way to render [`Mesh2d`] entities with custom shader logic.
///
/// Materials must implement [`AsBindGroup`] to define how data will be transferred to the GPU and bound in shaders.
/// [`AsBindGroup`] can be derived, which makes generating bindings straightforward. See the [`AsBindGroup`] docs for details.
///
/// # Example
///
/// Here is a simple [`Material2d`] implementation. The [`AsBindGroup`] derive has many features. To see what else is available,
/// check out the [`AsBindGroup`] documentation.
///
/// ```
/// # use bevy_sprite::{Material2d, MeshMaterial2d};
/// # use bevy_ecs::prelude::*;
/// # use bevy_image::Image;
/// # use bevy_reflect::TypePath;
/// # use bevy_render::{mesh::{Mesh, Mesh2d}, render_resource::{AsBindGroup, ShaderRef}};
/// # use bevy_color::LinearRgba;
/// # use bevy_color::palettes::basic::RED;
/// # use bevy_asset::{Handle, AssetServer, Assets, Asset};
/// # use bevy_math::primitives::Circle;
/// #
/// #[derive(AsBindGroup, Debug, Clone, Asset, TypePath)]
/// pub struct CustomMaterial {
///     // Uniform bindings must implement `ShaderType`, which will be used to convert the value to
///     // its shader-compatible equivalent. Most core math types already implement `ShaderType`.
///     #[uniform(0)]
///     color: LinearRgba,
///     // Images can be bound as textures in shaders. If the Image's sampler is also needed, just
///     // add the sampler attribute with a different binding index.
///     #[texture(1)]
///     #[sampler(2)]
///     color_texture: Handle<Image>,
/// }
///
/// // All functions on `Material2d` have default impls. You only need to implement the
/// // functions that are relevant for your material.
/// impl Material2d for CustomMaterial {
///     fn fragment_shader() -> ShaderRef {
///         "shaders/custom_material.wgsl".into()
///     }
/// }
///
/// // Spawn an entity with a mesh using `CustomMaterial`.
/// fn setup(
///     mut commands: Commands,
///     mut meshes: ResMut<Assets<Mesh>>,
///     mut materials: ResMut<Assets<CustomMaterial>>,
///     asset_server: Res<AssetServer>,
/// ) {
///     commands.spawn((
///         Mesh2d(meshes.add(Circle::new(50.0))),
///         MeshMaterial2d(materials.add(CustomMaterial {
///             color: RED.into(),
///             color_texture: asset_server.load("some_image.png"),
///         })),
///     ));
/// }
/// ```
///
/// In WGSL shaders, the material's binding would look like this:
///
/// ```wgsl
/// struct CustomMaterial {
///     color: vec4<f32>,
/// }
///
/// @group(2) @binding(0) var<uniform> material: CustomMaterial;
/// @group(2) @binding(1) var color_texture: texture_2d<f32>;
/// @group(2) @binding(2) var color_sampler: sampler;
/// ```
pub trait Material2d: AsBindGroup + Asset + Clone + Sized {
    /// Returns this material's vertex shader. If [`ShaderRef::Default`] is returned, the default mesh vertex shader
    /// will be used.
    fn vertex_shader() -> ShaderRef {
        ShaderRef::Default
    }

    /// Returns this material's fragment shader. If [`ShaderRef::Default`] is returned, the default mesh fragment shader
    /// will be used.
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Default
    }

    /// Add a bias to the view depth of the mesh which can be used to force a specific render order.
    #[inline]
    fn depth_bias(&self) -> f32 {
        0.0
    }

    fn alpha_mode(&self) -> AlphaMode2d {
        AlphaMode2d::Opaque
    }

    /// Customizes the default [`RenderPipelineDescriptor`].
    #[allow(unused_variables)]
    #[inline]
    fn specialize(
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        key: Material2dKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        Ok(())
    }
}

/// A [material](Material2d) used for rendering a [`Mesh2d`].
///
/// See [`Material2d`] for general information about 2D materials and how to implement your own materials.
///
/// # Example
///
/// ```
/// # use bevy_sprite::{ColorMaterial, MeshMaterial2d};
/// # use bevy_ecs::prelude::*;
/// # use bevy_render::mesh::{Mesh, Mesh2d};
/// # use bevy_color::palettes::basic::RED;
/// # use bevy_asset::Assets;
/// # use bevy_math::primitives::Circle;
/// #
/// // Spawn an entity with a mesh using `ColorMaterial`.
/// fn setup(
///     mut commands: Commands,
///     mut meshes: ResMut<Assets<Mesh>>,
///     mut materials: ResMut<Assets<ColorMaterial>>,
/// ) {
///     commands.spawn((
///         Mesh2d(meshes.add(Circle::new(50.0))),
///         MeshMaterial2d(materials.add(ColorMaterial::from_color(RED))),
///     ));
/// }
/// ```
///
/// [`MeshMaterial2d`]: crate::MeshMaterial2d
#[derive(Component, Clone, Debug, Deref, DerefMut, Reflect, PartialEq, Eq, From)]
#[reflect(Component, Default)]
pub struct MeshMaterial2d<M: Material2d>(pub Handle<M>);

impl<M: Material2d> Default for MeshMaterial2d<M> {
    fn default() -> Self {
        Self(Handle::default())
    }
}

impl<M: Material2d> From<MeshMaterial2d<M>> for AssetId<M> {
    fn from(material: MeshMaterial2d<M>) -> Self {
        material.id()
    }
}

impl<M: Material2d> From<&MeshMaterial2d<M>> for AssetId<M> {
    fn from(material: &MeshMaterial2d<M>) -> Self {
        material.id()
    }
}

/// Sets how a 2d material's base color alpha channel is used for transparency.
/// Currently, this only works with [`Mesh2d`]. Sprites are always transparent.
///
/// This is very similar to [`AlphaMode`](bevy_render::alpha::AlphaMode) but this only applies to 2d meshes.
/// We use a separate type because 2d doesn't support all the transparency modes that 3d does.
#[derive(Debug, Default, Reflect, Copy, Clone, PartialEq)]
#[reflect(Default, Debug)]
pub enum AlphaMode2d {
    /// Base color alpha values are overridden to be fully opaque (1.0).
    #[default]
    Opaque,
    /// Reduce transparency to fully opaque or fully transparent
    /// based on a threshold.
    ///
    /// Compares the base color alpha value to the specified threshold.
    /// If the value is below the threshold,
    /// considers the color to be fully transparent (alpha is set to 0.0).
    /// If it is equal to or above the threshold,
    /// considers the color to be fully opaque (alpha is set to 1.0).
    Mask(f32),
    /// The base color alpha value defines the opacity of the color.
    /// Standard alpha-blending is used to blend the fragment's color
    /// with the color behind it.
    Blend,
}

/// Adds the necessary ECS resources and render logic to enable rendering entities using the given [`Material2d`]
/// asset type (which includes [`Material2d`] types).
pub struct Material2dPlugin<M: Material2d>(PhantomData<M>);

impl<M: Material2d> Default for Material2dPlugin<M> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<M: Material2d> Plugin for Material2dPlugin<M>
where
    M::Data: PartialEq + Eq + Hash + Clone,
{
    fn build(&self, app: &mut App) {
        app.init_asset::<M>()
            .register_type::<MeshMaterial2d<M>>()
            .add_plugins(RenderAssetPlugin::<PreparedMaterial2d<M>>::default());

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .add_render_command::<Opaque2d, DrawMaterial2d<M>>()
                .add_render_command::<AlphaMask2d, DrawMaterial2d<M>>()
                .add_render_command::<Transparent2d, DrawMaterial2d<M>>()
                .init_resource::<RenderMaterial2dInstances<M>>()
                .init_resource::<SpecializedMeshPipelines<Material2dPipeline<M>>>()
                .add_systems(ExtractSchedule, extract_mesh_materials_2d::<M>)
                .add_systems(
                    Render,
                    queue_material2d_meshes::<M>
                        .in_set(RenderSet::QueueMeshes)
                        .after(prepare_assets::<PreparedMaterial2d<M>>),
                );
        }
    }

    fn finish(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.init_resource::<Material2dPipeline<M>>();
        }
    }
}

#[derive(Resource, Deref, DerefMut)]
pub struct RenderMaterial2dInstances<M: Material2d>(MainEntityHashMap<AssetId<M>>);

impl<M: Material2d> Default for RenderMaterial2dInstances<M> {
    fn default() -> Self {
        Self(Default::default())
    }
}

pub fn extract_mesh_materials_2d<M: Material2d>(
    mut material_instances: ResMut<RenderMaterial2dInstances<M>>,
    query: Extract<Query<(Entity, &ViewVisibility, &MeshMaterial2d<M>), With<Mesh2d>>>,
) {
    material_instances.clear();

    for (entity, view_visibility, material) in &query {
        if view_visibility.get() {
            material_instances.insert(entity.into(), material.id());
        }
    }
}

/// Render pipeline data for a given [`Material2d`]
#[derive(Resource)]
pub struct Material2dPipeline<M: Material2d> {
    pub mesh2d_pipeline: Mesh2dPipeline,
    pub material2d_layout: BindGroupLayout,
    pub vertex_shader: Option<Handle<Shader>>,
    pub fragment_shader: Option<Handle<Shader>>,
    marker: PhantomData<M>,
}

pub struct Material2dKey<M: Material2d> {
    pub mesh_key: Mesh2dPipelineKey,
    pub bind_group_data: M::Data,
}

impl<M: Material2d> Eq for Material2dKey<M> where M::Data: PartialEq {}

impl<M: Material2d> PartialEq for Material2dKey<M>
where
    M::Data: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.mesh_key == other.mesh_key && self.bind_group_data == other.bind_group_data
    }
}

impl<M: Material2d> Clone for Material2dKey<M>
where
    M::Data: Clone,
{
    fn clone(&self) -> Self {
        Self {
            mesh_key: self.mesh_key,
            bind_group_data: self.bind_group_data.clone(),
        }
    }
}

impl<M: Material2d> Hash for Material2dKey<M>
where
    M::Data: Hash,
{
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.mesh_key.hash(state);
        self.bind_group_data.hash(state);
    }
}

impl<M: Material2d> Clone for Material2dPipeline<M> {
    fn clone(&self) -> Self {
        Self {
            mesh2d_pipeline: self.mesh2d_pipeline.clone(),
            material2d_layout: self.material2d_layout.clone(),
            vertex_shader: self.vertex_shader.clone(),
            fragment_shader: self.fragment_shader.clone(),
            marker: PhantomData,
        }
    }
}

impl<M: Material2d> SpecializedMeshPipeline for Material2dPipeline<M>
where
    M::Data: PartialEq + Eq + Hash + Clone,
{
    type Key = Material2dKey<M>;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh2d_pipeline.specialize(key.mesh_key, layout)?;
        if let Some(vertex_shader) = &self.vertex_shader {
            descriptor.vertex.shader = vertex_shader.clone();
        }

        if let Some(fragment_shader) = &self.fragment_shader {
            descriptor.fragment.as_mut().unwrap().shader = fragment_shader.clone();
        }
        descriptor.layout = vec![
            self.mesh2d_pipeline.view_layout.clone(),
            self.mesh2d_pipeline.mesh_layout.clone(),
            self.material2d_layout.clone(),
        ];

        M::specialize(&mut descriptor, layout, key)?;
        Ok(descriptor)
    }
}

impl<M: Material2d> FromWorld for Material2dPipeline<M> {
    fn from_world(world: &mut World) -> Self {
        let asset_server = world.resource::<AssetServer>();
        let render_device = world.resource::<RenderDevice>();
        let material2d_layout = M::bind_group_layout(render_device);

        Material2dPipeline {
            mesh2d_pipeline: world.resource::<Mesh2dPipeline>().clone(),
            material2d_layout,
            vertex_shader: match M::vertex_shader() {
                ShaderRef::Default => None,
                ShaderRef::Handle(handle) => Some(handle),
                ShaderRef::Path(path) => Some(asset_server.load(path)),
            },
            fragment_shader: match M::fragment_shader() {
                ShaderRef::Default => None,
                ShaderRef::Handle(handle) => Some(handle),
                ShaderRef::Path(path) => Some(asset_server.load(path)),
            },
            marker: PhantomData,
        }
    }
}

pub(super) type DrawMaterial2d<M> = (
    SetItemPipeline,
    SetMesh2dViewBindGroup<0>,
    SetMesh2dBindGroup<1>,
    SetMaterial2dBindGroup<M, 2>,
    DrawMesh2d,
);

pub struct SetMaterial2dBindGroup<M: Material2d, const I: usize>(PhantomData<M>);
impl<P: PhaseItem, M: Material2d, const I: usize> RenderCommand<P>
    for SetMaterial2dBindGroup<M, I>
{
    type Param = (
        SRes<RenderAssets<PreparedMaterial2d<M>>>,
        SRes<RenderMaterial2dInstances<M>>,
    );
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _item_query: Option<()>,
        (materials, material_instances): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let materials = materials.into_inner();
        let material_instances = material_instances.into_inner();
        let Some(material_instance) = material_instances.get(&item.main_entity()) else {
            return RenderCommandResult::Skip;
        };
        let Some(material2d) = materials.get(*material_instance) else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(I, &material2d.bind_group, &[]);
        RenderCommandResult::Success
    }
}

pub const fn alpha_mode_pipeline_key(alpha_mode: AlphaMode2d) -> Mesh2dPipelineKey {
    match alpha_mode {
        AlphaMode2d::Blend => Mesh2dPipelineKey::BLEND_ALPHA,
        AlphaMode2d::Mask(_) => Mesh2dPipelineKey::MAY_DISCARD,
        _ => Mesh2dPipelineKey::NONE,
    }
}

pub const fn tonemapping_pipeline_key(tonemapping: Tonemapping) -> Mesh2dPipelineKey {
    match tonemapping {
        Tonemapping::None => Mesh2dPipelineKey::TONEMAP_METHOD_NONE,
        Tonemapping::Reinhard => Mesh2dPipelineKey::TONEMAP_METHOD_REINHARD,
        Tonemapping::ReinhardLuminance => Mesh2dPipelineKey::TONEMAP_METHOD_REINHARD_LUMINANCE,
        Tonemapping::AcesFitted => Mesh2dPipelineKey::TONEMAP_METHOD_ACES_FITTED,
        Tonemapping::AgX => Mesh2dPipelineKey::TONEMAP_METHOD_AGX,
        Tonemapping::SomewhatBoringDisplayTransform => {
            Mesh2dPipelineKey::TONEMAP_METHOD_SOMEWHAT_BORING_DISPLAY_TRANSFORM
        }
        Tonemapping::TonyMcMapface => Mesh2dPipelineKey::TONEMAP_METHOD_TONY_MC_MAPFACE,
        Tonemapping::BlenderFilmic => Mesh2dPipelineKey::TONEMAP_METHOD_BLENDER_FILMIC,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn queue_material2d_meshes<M: Material2d>(
    opaque_draw_functions: Res<DrawFunctions<Opaque2d>>,
    alpha_mask_draw_functions: Res<DrawFunctions<AlphaMask2d>>,
    transparent_draw_functions: Res<DrawFunctions<Transparent2d>>,
    material2d_pipeline: Res<Material2dPipeline<M>>,
    mut pipelines: ResMut<SpecializedMeshPipelines<Material2dPipeline<M>>>,
    pipeline_cache: Res<PipelineCache>,
    render_meshes: Res<RenderAssets<RenderMesh>>,
    render_materials: Res<RenderAssets<PreparedMaterial2d<M>>>,
    mut render_mesh_instances: ResMut<RenderMesh2dInstances>,
    render_material_instances: Res<RenderMaterial2dInstances<M>>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<Transparent2d>>,
    mut opaque_render_phases: ResMut<ViewBinnedRenderPhases<Opaque2d>>,
    mut alpha_mask_render_phases: ResMut<ViewBinnedRenderPhases<AlphaMask2d>>,
    views: Query<(
        Entity,
        &ExtractedView,
        &RenderVisibleEntities,
        &Msaa,
        Option<&Tonemapping>,
        Option<&DebandDither>,
    )>,
) where
    M::Data: PartialEq + Eq + Hash + Clone,
{
    if render_material_instances.is_empty() {
        return;
    }

    for (view_entity, view, visible_entities, msaa, tonemapping, dither) in &views {
        let Some(transparent_phase) = transparent_render_phases.get_mut(&view_entity) else {
            continue;
        };
        let Some(opaque_phase) = opaque_render_phases.get_mut(&view_entity) else {
            continue;
        };
        let Some(alpha_mask_phase) = alpha_mask_render_phases.get_mut(&view_entity) else {
            continue;
        };

        let draw_transparent_2d = transparent_draw_functions.read().id::<DrawMaterial2d<M>>();
        let draw_opaque_2d = opaque_draw_functions.read().id::<DrawMaterial2d<M>>();
        let draw_alpha_mask_2d = alpha_mask_draw_functions.read().id::<DrawMaterial2d<M>>();

        let mut view_key = Mesh2dPipelineKey::from_msaa_samples(msaa.samples())
            | Mesh2dPipelineKey::from_hdr(view.hdr);

        if !view.hdr {
            if let Some(tonemapping) = tonemapping {
                view_key |= Mesh2dPipelineKey::TONEMAP_IN_SHADER;
                view_key |= tonemapping_pipeline_key(*tonemapping);
            }
            if let Some(DebandDither::Enabled) = dither {
                view_key |= Mesh2dPipelineKey::DEBAND_DITHER;
            }
        }
        for (render_entity, visible_entity) in visible_entities.iter::<With<Mesh2d>>() {
            let Some(material_asset_id) = render_material_instances.get(visible_entity) else {
                continue;
            };
            let Some(mesh_instance) = render_mesh_instances.get_mut(visible_entity) else {
                continue;
            };
            let Some(material_2d) = render_materials.get(*material_asset_id) else {
                continue;
            };
            let Some(mesh) = render_meshes.get(mesh_instance.mesh_asset_id) else {
                continue;
            };
            let mesh_key = view_key
                | Mesh2dPipelineKey::from_primitive_topology(mesh.primitive_topology())
                | material_2d.properties.mesh_pipeline_key_bits;

            let pipeline_id = pipelines.specialize(
                &pipeline_cache,
                &material2d_pipeline,
                Material2dKey {
                    mesh_key,
                    bind_group_data: material_2d.key.clone(),
                },
                &mesh.layout,
            );

            let pipeline_id = match pipeline_id {
                Ok(id) => id,
                Err(err) => {
                    error!("{}", err);
                    continue;
                }
            };

            mesh_instance.material_bind_group_id = material_2d.get_bind_group_id();
            let mesh_z = mesh_instance.transforms.world_from_local.translation.z;

            match material_2d.properties.alpha_mode {
                AlphaMode2d::Opaque => {
                    let bin_key = Opaque2dBinKey {
                        pipeline: pipeline_id,
                        draw_function: draw_opaque_2d,
                        asset_id: mesh_instance.mesh_asset_id.into(),
                        material_bind_group_id: material_2d.get_bind_group_id().0,
                    };
                    opaque_phase.add(
                        bin_key,
                        (*render_entity, *visible_entity),
                        BinnedRenderPhaseType::mesh(mesh_instance.automatic_batching),
                    );
                }
                AlphaMode2d::Mask(_) => {
                    let bin_key = AlphaMask2dBinKey {
                        pipeline: pipeline_id,
                        draw_function: draw_alpha_mask_2d,
                        asset_id: mesh_instance.mesh_asset_id.into(),
                        material_bind_group_id: material_2d.get_bind_group_id().0,
                    };
                    alpha_mask_phase.add(
                        bin_key,
                        (*render_entity, *visible_entity),
                        BinnedRenderPhaseType::mesh(mesh_instance.automatic_batching),
                    );
                }
                AlphaMode2d::Blend => {
                    transparent_phase.add(Transparent2d {
                        entity: (*render_entity, *visible_entity),
                        draw_function: draw_transparent_2d,
                        pipeline: pipeline_id,
                        // NOTE: Back-to-front ordering for transparent with ascending sort means far should have the
                        // lowest sort key and getting closer should increase. As we have
                        // -z in front of the camera, the largest distance is -far with values increasing toward the
                        // camera. As such we can just use mesh_z as the distance
                        sort_key: FloatOrd(mesh_z + material_2d.properties.depth_bias),
                        // Batching is done in batch_and_prepare_render_phase
                        batch_range: 0..1,
                        extra_index: PhaseItemExtraIndex::NONE,
                    });
                }
            }
        }
    }
}

#[derive(Component, Clone, Copy, Default, PartialEq, Eq, Deref, DerefMut)]
pub struct Material2dBindGroupId(pub Option<BindGroupId>);

/// Common [`Material2d`] properties, calculated for a specific material instance.
pub struct Material2dProperties {
    /// The [`AlphaMode2d`] of this material.
    pub alpha_mode: AlphaMode2d,
    /// Add a bias to the view depth of the mesh which can be used to force a specific render order
    /// for meshes with equal depth, to avoid z-fighting.
    /// The bias is in depth-texture units so large values may
    pub depth_bias: f32,
    /// The bits in the [`Mesh2dPipelineKey`] for this material.
    ///
    /// These are precalculated so that we can just "or" them together in
    /// [`queue_material2d_meshes`].
    pub mesh_pipeline_key_bits: Mesh2dPipelineKey,
}

/// Data prepared for a [`Material2d`] instance.
pub struct PreparedMaterial2d<T: Material2d> {
    pub bindings: Vec<(u32, OwnedBindingResource)>,
    pub bind_group: BindGroup,
    pub key: T::Data,
    pub properties: Material2dProperties,
}

impl<T: Material2d> PreparedMaterial2d<T> {
    pub fn get_bind_group_id(&self) -> Material2dBindGroupId {
        Material2dBindGroupId(Some(self.bind_group.id()))
    }
}

impl<M: Material2d> RenderAsset for PreparedMaterial2d<M> {
    type SourceAsset = M;

    type Param = (SRes<RenderDevice>, SRes<Material2dPipeline<M>>, M::Param);

    fn prepare_asset(
        material: Self::SourceAsset,
        (render_device, pipeline, material_param): &mut SystemParamItem<Self::Param>,
    ) -> Result<Self, PrepareAssetError<Self::SourceAsset>> {
        match material.as_bind_group(&pipeline.material2d_layout, render_device, material_param) {
            Ok(prepared) => {
                let mut mesh_pipeline_key_bits = Mesh2dPipelineKey::empty();
                mesh_pipeline_key_bits.insert(alpha_mode_pipeline_key(material.alpha_mode()));
                Ok(PreparedMaterial2d {
                    bindings: prepared.bindings,
                    bind_group: prepared.bind_group,
                    key: prepared.data,
                    properties: Material2dProperties {
                        depth_bias: material.depth_bias(),
                        alpha_mode: material.alpha_mode(),
                        mesh_pipeline_key_bits,
                    },
                })
            }
            Err(AsBindGroupError::RetryNextUpdate) => {
                Err(PrepareAssetError::RetryNextUpdate(material))
            }
            Err(other) => Err(PrepareAssetError::AsBindGroupError(other)),
        }
    }
}

/// A component bundle for entities with a [`Mesh2d`] and a [`MeshMaterial2d`].
#[derive(Bundle, Clone)]
#[deprecated(
    since = "0.15.0",
    note = "Use the `Mesh2d` and `MeshMaterial2d` components instead. Inserting them will now also insert the other components required by them automatically."
)]
pub struct MaterialMesh2dBundle<M: Material2d> {
    pub mesh: Mesh2d,
    pub material: MeshMaterial2d<M>,
    pub transform: Transform,
    pub global_transform: GlobalTransform,
    /// User indication of whether an entity is visible
    pub visibility: Visibility,
    // Inherited visibility of an entity.
    pub inherited_visibility: InheritedVisibility,
    // Indication of whether an entity is visible in any view.
    pub view_visibility: ViewVisibility,
}

impl<M: Material2d> Default for MaterialMesh2dBundle<M> {
    fn default() -> Self {
        Self {
            mesh: Default::default(),
            material: Default::default(),
            transform: Default::default(),
            global_transform: Default::default(),
            visibility: Default::default(),
            inherited_visibility: Default::default(),
            view_visibility: Default::default(),
        }
    }
}
