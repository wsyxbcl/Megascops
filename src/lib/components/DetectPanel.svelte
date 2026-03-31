<script lang="ts">
    import { open } from "@tauri-apps/plugin-shell";
    import { Badge } from "$lib/components/ui/badge/index";
    import { Toggle } from "$lib/components/ui/toggle/index";
    import * as Card from "$lib/components/ui/card/index";
    import { Label } from "$lib/components/ui/label/index";
    import { Button } from "$lib/components/ui/button";
    import { Input } from "$lib/components/ui/input";
    import StatusIndicator from "$lib/components/StatusIndicator.svelte";
    import TooltipWrapper from "$lib/components/TooltipWrapper.svelte";
    import { _ } from "svelte-i18n";
    import {
        Folder,
        Bolt,
        Play,
        LoaderCircle,
        EyeOff,
        Eye,
        Shapes,
        Undo2,
        FlaskConical,
        CircleHelp,
        Clock,
        Github,
    } from "lucide-svelte";
    import {
        selectFolder,
        selectResumePath,
        startProcessing,
        organize,
        undo,
        toggleConfig,
        formatQuota,
    } from "$lib/utils";
    import { detectStatus, config } from "$lib/store.svelte";
    import { onDestroy } from "svelte";
    import { startTour } from "$lib/tour";

    let elapsedTime = $state("00:00:00");
    let remainingTime = $state("");
    let timerInterval: ReturnType<typeof setInterval> | null = null;
    let startTime: number | null = null;
    let lastProgress = 0;

    // Format seconds into HH:MM:SS
    function formatTime(seconds: number): string {
        const hours = Math.floor(seconds / 3600);
        const minutes = Math.floor((seconds % 3600) / 60);
        const secs = Math.floor(seconds % 60);

        return [hours, minutes, secs]
            .map((v) => v.toString().padStart(2, "0"))
            .join(":");
    }

    // Start timer when processing begins
    function startTimer() {
        if (timerInterval) return;

        startTime = Date.now();
        lastProgress = detectStatus.progress;

        timerInterval = setInterval(() => {
            const elapsed = Math.floor((Date.now() - startTime!) / 1000);
            elapsedTime = formatTime(elapsed);

            const currentProgress = detectStatus.progress;
            if (currentProgress > 0 && currentProgress > lastProgress) {
                const progressRate = currentProgress / elapsed;
                if (progressRate > 0) {
                    const remainingSeconds = Math.max(
                        0,
                        Math.floor((100 - currentProgress) / progressRate),
                    );
                    remainingTime = formatTime(remainingSeconds);
                }
            }
            lastProgress = currentProgress;
        }, 1000);
    }

    function stopTimer() {
        if (timerInterval) {
            clearInterval(timerInterval);
            timerInterval = null;
            remainingTime = "";
        }
    }

    async function handleStartProcessing() {
        startTimer();
        await startProcessing();
    }

    // Clean up on component destroy
    onDestroy(() => {
        stopTimer();
    });

    // Watch for changes in processing status
    $effect(() => {
        if (!detectStatus.isProcessing && timerInterval) {
            stopTimer();
        }
    });

    const formattedQuota = $derived(formatQuota(detectStatus.quota));

    function openGithub() {
        open("https://github.com/wsyxbcl/Megascops");
    }
</script>

<Card.Root class="h-full w-full m-0 rounded-none shadow-none">
    <Card.Header class="flex justify-between items-center flex-row">
        <Card.Title>{$_("title.detect")}</Card.Title>
        <div>
            <TooltipWrapper text={$_("tooltip.github")}>
                <Button
                    id="github"
                    variant="ghost"
                    size="icon"
                    onclick={openGithub}
                >
                    <Github style="width: 1.5rem; height: 1.5rem;" /></Button
                >
            </TooltipWrapper>
            <TooltipWrapper text={$_("tooltip.help")}>
                <Button
                    id="help"
                    variant="ghost"
                    size="icon"
                    onclick={startTour}
                    disabled={detectStatus.isProcessing}
                >
                    <CircleHelp style="width: 1.5rem; height: 1.5rem;" />
                </Button>
            </TooltipWrapper>
            <TooltipWrapper text={$_("tooltip.config")}>
                <Button
                    id="config-button"
                    variant="ghost"
                    size="icon"
                    onclick={toggleConfig}
                    disabled={detectStatus.isProcessing}
                    class="config-button"
                >
                    <div
                        class={detectStatus.configIconAnimating
                            ? "spin-animation-open"
                            : ""}
                    >
                        <Bolt style="width: 1.5rem; height: 1.5rem;" />
                    </div>
                </Button>
            </TooltipWrapper>
        </div>
    </Card.Header>
    <Card.Content class="flex flex-col gap-6">
        <section class="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div id="media-folder" class="flex flex-col gap-2 col-span-full">
                <Label for="folder">{$_("detect.folder")}</Label>
                <div class="flex gap-2">
                    <Input
                        type="text"
                        id="folder"
                        bind:value={config.detectOptions.selectedFolder}
                        placeholder={$_("detect.folderPlaceholder")}
                    />
                    <Button
                        variant="outline"
                        size="icon"
                        onclick={selectFolder}
                        disabled={detectStatus.isProcessing}
                    >
                        <Folder />
                    </Button>
                </div>
            </div>
            <div id="grpc-url" class="flex flex-col gap-2 col-span-full">
                <Label>{$_("detect.url")}</Label>
                <div class="flex items-center gap-2">
                    <div class="flex-grow">
                        <Input
                            type="text"
                            bind:value={config.detectOptions.grpcUrl}
                        />
                    </div>
                    <div id="service-status" class="m-3">
                        <StatusIndicator status={detectStatus.serviceStatus} />
                    </div>
                </div>
            </div>

            <div id="access-token" class="flex flex-col gap-2">
                <Label>{$_("detect.token")}</Label>
                <div class="flex items-center gap-2">
                    <div class="relative flex-grow">
                        <Input
                            type={detectStatus.showPassword
                                ? "text"
                                : "password"}
                            bind:value={config.detectOptions.accessToken}
                            placeholder=""
                            class="pr-10 w-full"
                        />
                        <Button
                            type="button"
                            variant="ghost"
                            class="absolute right-0 top-0 h-full px-3 hover:bg-transparent"
                            onclick={() =>
                                (detectStatus.showPassword =
                                    !detectStatus.showPassword)}
                        >
                            {#if detectStatus.showPassword}
                                <Eye class="h-4 w-4" />
                            {:else}
                                <EyeOff class="h-4 w-4" />
                            {/if}
                        </Button>
                    </div>
                    <TooltipWrapper
                        text={formattedQuota !== "invalid"
                            ? `${$_("tooltip.quota")}: ${detectStatus.quota}`
                            : $_("tooltip.tokenInvalid")}
                    >
                        <Badge
                            variant={formattedQuota === "invalid"
                                ? "destructive"
                                : "default"}
                            id="quota">{formattedQuota}</Badge
                        >
                    </TooltipWrapper>
                </div>
            </div>

            <div id="resume-path" class="flex flex-col gap-2">
                <Label>{$_("detect.resumePath")}</Label>
                <div class="flex gap-2">
                    <Input
                        type="text"
                        bind:value={config.detectOptions.resumePath}
                        placeholder={$_("detect.resumePathPlaceholder")}
                    />
                    <Button
                        variant="outline"
                        size="icon"
                        onclick={selectResumePath}
                        disabled={detectStatus.isProcessing}
                    >
                        <Folder />
                    </Button>
                </div>
            </div>
        </section>
        <div id="progress" class="mb-0 relative">
            <!-- 进度条 -->
            <div class="h-5 bg-muted rounded-full overflow-hidden">
                <div
                    class="h-5 bg-primary flex items-center justify-center transition-all duration-300 ease-out"
                    style="width: {detectStatus.progress}%"
                >
                    <span class="text-xs font-medium text-primary-foreground">
                        {detectStatus.progress.toFixed(2)}%
                    </span>
                </div>
            </div>

            <!-- 时间信息 -->
            <div
                class="flex justify-between mt-2 text-xs text-muted-foreground"
            >
                <div class="flex items-center gap-1">
                    <Clock class="h-3 w-3" />
                    <span>{elapsedTime}</span>
                </div>

                {#if remainingTime !== ""}
                    <div>
                        {$_("detect.remainTime")}
                        {remainingTime}
                    </div>
                {/if}
            </div>
        </div>
        <div class="flex items-center relative -mt-2">
            <TooltipWrapper text={$_("tooltip.guess")}>
                <Toggle
                    id="guess"
                    size="sm"
                    aria-label="Toggle guess"
                    bind:pressed={config.detectOptions.guess}
                >
                    <FlaskConical class="h-4 w-4" />
                </Toggle>
            </TooltipWrapper>

            <div
                class="absolute left-1/2 transform -translate-x-1/2 flex items-center gap-2"
            >
                <div class="flex items-center">
                    <TooltipWrapper text={$_("tooltip.organize")}>
                        <Button
                            id="organize"
                            variant="ghost"
                            size="icon"
                            onclick={organize}
                            disabled={detectStatus.isProcessing ||
                                !config.detectOptions.selectedFolder}
                        >
                            {#if detectStatus.isOrganizing}
                                <LoaderCircle
                                    class="animate-spin"
                                    style="width: 1.2rem; height: 1.2rem;"
                                />
                            {:else}
                                <Shapes
                                    style="width: 1.2rem; height: 1.2rem;"
                                />
                            {/if}
                        </Button>
                    </TooltipWrapper>

                    <TooltipWrapper text={$_("tooltip.start")}>
                        <Button
                            id="start"
                            variant="ghost"
                            size="icon"
                            onclick={handleStartProcessing}
                            disabled={detectStatus.isProcessing ||
                                !config.detectOptions.selectedFolder ||
                                !config.detectOptions.grpcUrl ||
                                !config.detectOptions.accessToken ||
                                detectStatus.serviceStatus !== "online"}
                        >
                            {#if detectStatus.isProcessing}
                                <LoaderCircle
                                    class="animate-spin"
                                    style="width: 1.5rem; height: 1.5rem;"
                                />
                            {:else}
                                <Play style="width: 1.5rem; height: 1.5rem;" />
                            {/if}
                        </Button>
                    </TooltipWrapper>

                    <TooltipWrapper text={$_("tooltip.undo")}>
                        <Button
                            id="undo"
                            variant="ghost"
                            size="icon"
                            onclick={undo}
                            disabled={detectStatus.isProcessing ||
                                !config.detectOptions.selectedFolder}
                        >
                            {#if detectStatus.isUndoOrganizing}
                                <LoaderCircle
                                    class="animate-spin"
                                    style="width: 1.2rem; height: 1.2rem;"
                                />
                            {:else}
                                <Undo2 style="width: 1.2rem; height: 1.2rem;" />
                            {/if}
                        </Button>
                    </TooltipWrapper>
                </div>
            </div>
        </div>
    </Card.Content>
</Card.Root>

<style>
    @keyframes spin {
        0% {
            transform: rotate(0deg);
        }
        100% {
            transform: rotate(360deg);
        }
    }

    @keyframes spin-open {
        0% {
            transform: rotate(0deg);
        }
        100% {
            transform: rotate(-180deg);
        }
    }

    .spin-animation-open {
        animation: spin-open 0.5s ease-in-out;
    }
</style>
