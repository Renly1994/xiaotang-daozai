//! 投放态视觉 —— 拖拽进入岛体时的高亮描边 + 「松手收纳」提示。
//! 由 SyncState.dropHot 驱动；岛体强制展开见 island_widget 集成。
import 'package:flutter/material.dart';
import '../core/theme.dart';

class DropOverlay extends StatelessWidget {
  final bool hot;
  final Widget child;
  const DropOverlay({super.key, required this.hot, required this.child});

  @override
  Widget build(BuildContext context) {
    return Stack(
      alignment: Alignment.center,
      children: [
        child,
        if (hot)
          Positioned.fill(
            child: IgnorePointer(
              child: Container(
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(DaozaiGeom.rad),
                  border: Border.all(color: DaozaiColors.cyan, width: 2),
                  color: DaozaiColors.alphaBlue(0.12),
                ),
                alignment: Alignment.center,
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                  decoration: BoxDecoration(
                    color: DaozaiColors.card,
                    borderRadius: BorderRadius.circular(8),
                  ),
                  child: const Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(Icons.file_download_outlined, size: 16, color: DaozaiColors.cyan),
                      SizedBox(width: 6),
                      Text('松手收纳', style: TextStyle(fontSize: DaozaiSize.body, color: DaozaiColors.cyan, fontWeight: FontWeight.w600)),
                    ],
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}